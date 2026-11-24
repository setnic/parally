mod http;
mod storage;

use anyhow::{Context, Result, anyhow, bail, ensure};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
    thread,
    time::{Duration, Instant},
};

use http::{RangeIssue, Retryable};
use storage::{Job, Manifest, Part, Remote};

pub const MAX_STREAMS: usize = 16;
const ATTEMPTS: usize = 4;
// A range connection that delivered this much before failing earns a fresh set of attempts.
const PROGRESS_RESET: u64 = 1024 * 1024;
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(2);
const BLOCK: usize = 64 * 1024;

#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Download paused.")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Clone, Default)]
pub struct Progress {
    pub bytes: u64,
    pub total: Option<u64>,
    pub status: String,
    pub segments: Vec<SegmentProgress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentProgress {
    pub length: u64,
    pub bytes: u64,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    #[default]
    Normal,
    Resume,
    Restart,
}

#[derive(Debug)]
pub struct RestartRequired(pub &'static str);
impl std::fmt::Display for RestartRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for RestartRequired {}

/// Cookie and Referer the browser had for one download. Parally keeps them in memory only.
#[derive(Clone, Default)]
pub struct BrowserHeaders {
    pub cookie: String,
    pub referrer: String,
}

#[derive(Default)]
pub struct Download {
    cancelled: AtomicBool,
    connections: AtomicUsize,
    commit: Mutex<bool>,
    progress: Mutex<Progress>,
    headers: BrowserHeaders,
    mode: StartMode,
}

impl Download {
    pub fn set_connections(&self, streams: usize) {
        self.connections
            .store(streams.clamp(1, MAX_STREAMS), Relaxed);
    }
    pub fn with_headers(mut self, headers: BrowserHeaders) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_mode(mode: StartMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn cancel(&self) -> bool {
        let published = self.commit.lock().unwrap();
        if *published {
            return false;
        }
        self.cancelled.store(true, Relaxed);
        true
    }

    pub fn progress(&self) -> Progress {
        self.progress.lock().unwrap().clone()
    }

    fn check(&self) -> Result<()> {
        ensure!(!self.cancelled.load(Relaxed), Cancelled);
        Ok(())
    }

    fn update(&self, bytes: u64, total: Option<u64>, status: impl Into<String>) {
        *self.progress.lock().unwrap() = Progress {
            bytes,
            total,
            status: status.into(),
            segments: Vec::new(),
        };
    }

    fn add(&self, bytes: usize, segment: Option<usize>) {
        let mut progress = self.progress.lock().unwrap();
        progress.bytes += bytes as u64;
        if let Some(segment) = segment.and_then(|index| progress.segments.get_mut(index)) {
            segment.bytes += bytes as u64;
        }
    }

    fn retry_delay(&self, attempt: usize) -> Result<()> {
        // 0.5 s, 1 s, 2 s, ...
        for _ in 0..5usize << (attempt - 1).min(5) {
            self.check()?;
            thread::sleep(Duration::from_millis(100));
        }
        self.check()
    }

    pub fn run(&self, url: &str, destination: &Path, streams: usize) -> Result<()> {
        let url = validate_url(url)?;
        ensure!(
            (1..=MAX_STREAMS).contains(&streams),
            "Choose 1 to {MAX_STREAMS} connections."
        );
        self.check()?;
        let _ = self
            .connections
            .compare_exchange(0, streams, Relaxed, Relaxed);
        let job = Job::open(destination, url.as_str(), streams, self.mode)?;
        if self.mode == StartMode::Resume {
            let mut saved = job.checkpoint_progress()?;
            if streams == 1 {
                saved.segments.clear();
            }
            saved.status = "Checking the server…".into();
            *self.progress.lock().unwrap() = saved;
        } else {
            self.update(0, None, "Checking the server…");
        }
        let allow_html = destination
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "html" | "htm" | "xhtml"
                )
            });
        let transfer = self.transfer(&http::client(&self.headers), url.as_str(), &job, streams, allow_html);
        self.check()?;
        transfer?;
        if !allow_html {
            let mut prefix = Vec::new();
            job.data()?.take(1024).read_to_end(&mut prefix)?;
            let prefix = String::from_utf8_lossy(&prefix);
            let prefix = prefix
                .trim_start_matches('\u{feff}')
                .trim_start()
                .to_ascii_lowercase();
            ensure!(
                !["<!doctype html", "<html>", "<html ", "<head>", "<body>"]
                    .iter()
                    .any(|signature| prefix.starts_with(signature)),
                "The response body is an HTML page. Copy the direct file download link."
            );
        }
        let mut published = self.commit.lock().unwrap();
        self.check()?;
        job.publish(destination)?;
        *published = true;
        drop(published);
        job.cleanup();
        Ok(())
    }

    fn transfer(
        &self,
        agent: &ureq::Agent,
        url: &str,
        job: &Job,
        streams: usize,
        allow_html: bool,
    ) -> Result<()> {
        for attempt in 1..=ATTEMPTS {
            self.check()?;
            let result = http::probe(agent, url, allow_html);
            match result {
                Ok(http::Probe::Range(remote)) => {
                    job.prepare_ranges(remote, streams, self.mode)?;
                    match self.parallel(agent, job) {
                        Err(error) if error.is::<RangeIssue>() => {
                            if self.mode == StartMode::Resume {
                                return Err(RestartRequired(
                                    "The server stopped honoring the saved byte ranges.",
                                )
                                .into());
                            }
                            return self.single(agent, url, job, None, allow_html);
                        }
                        result => return result,
                    }
                }
                Ok(http::Probe::Full(response)) => {
                    if self.mode == StartMode::Resume {
                        return Err(RestartRequired(
                            "The server did not confirm byte ranges and a strong ETag.",
                        )
                        .into());
                    }
                    return self.single(agent, url, job, response, allow_html);
                }
                Err(error) if error.is::<Retryable>() && attempt < ATTEMPTS => {
                    self.retry_delay(attempt)?;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!()
    }

    fn parallel(&self, agent: &ureq::Agent, job: &Job) -> Result<()> {
        loop {
            self.check()?;
            let streams = self.connections.load(Relaxed);
            let manifest = job.balance(streams)?;
            if !self.range_batch(agent, job, &manifest, streams)? {
                return Ok(());
            }
        }
    }

    fn range_batch(
        &self,
        agent: &ureq::Agent,
        job: &Job,
        manifest: &Manifest,
        streams: usize,
    ) -> Result<bool> {
        let remote = manifest.remote.as_ref().unwrap();
        let pending: Vec<_> = manifest
            .parts
            .iter()
            .enumerate()
            .filter(|(_, part)| part.next < part.end)
            .collect();
        let workers = streams.min(pending.len());
        let mut state = manifest.progress(remote.size);
        let action = if state.bytes > 0 {
            "Resuming"
        } else {
            "Downloading"
        };
        state.status = format!("{action} · {workers} active connections");
        if workers <= 1 {
            state.segments.clear();
        }
        *self.progress.lock().unwrap() = state;
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let rebalance = AtomicBool::new(false);
        let should_stop = || {
            if self.connections.load(Relaxed) != streams {
                rebalance.store(true, Relaxed);
            }
            stop.load(Relaxed) || rebalance.load(Relaxed)
        };
        thread::scope(|scope| {
            let mut tasks = Vec::with_capacity(workers);
            for _ in 0..workers {
                tasks.push(scope.spawn(|| -> Result<()> {
                    while let Some(&(index, part)) = pending.get(next.fetch_add(1, Relaxed)) {
                        if should_stop() {
                            break;
                        }
                        let result = self.segment(agent, job, remote, index, *part, &should_stop);
                        if result.as_ref().is_err_and(|error| error.is::<RangeIssue>()) {
                            stop.store(true, Relaxed);
                        }
                        result?;
                    }
                    Ok(())
                }));
            }
            let mut failure = None;
            for task in tasks {
                if let Err(error) = task
                    .join()
                    .map_err(|_| anyhow!("A download worker stopped unexpectedly."))?
                    && (failure.is_none() || error.is::<RangeIssue>())
                {
                    failure = Some(error);
                }
            }
            self.check()?;
            failure.map_or_else(|| Ok(rebalance.load(Relaxed)), Err)
        })
    }

    fn segment(
        &self,
        agent: &ureq::Agent,
        job: &Job,
        remote: &Remote,
        index: usize,
        mut part: Part,
        should_stop: &(impl Fn() -> bool + Sync),
    ) -> Result<()> {
        let mut data = job.data()?;
        let mut buffer = vec![0; BLOCK];
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.check()?;
            if should_stop() {
                return Ok(());
            }
            let started = part.next;
            let mut saved = part.next;
            let mut last_checkpoint = Instant::now();
            let result = (|| -> Result<()> {
                let mut input = http::segment(agent, remote, part.next, part.end)?.into_reader();
                data.seek(SeekFrom::Start(part.next))?;
                while part.next < part.end {
                    self.check()?;
                    if should_stop() {
                        return Ok(());
                    }
                    let limit = (part.end - part.next).min(BLOCK as u64) as usize;
                    let length = read_network(&mut input, &mut buffer[..limit])?;
                    if length == 0 {
                        return Err(
                            Retryable("The server ended a range response early.".into()).into()
                        );
                    }
                    if part.next + length as u64 == part.end
                        && read_network(&mut input, &mut [0])? != 0
                    {
                        return Err(RangeIssue.into());
                    }
                    data.write_all(&buffer[..length])
                        .context("Could not write partial data")?;
                    part.next += length as u64;
                    self.add(length, Some(index));
                    if last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL {
                        job.checkpoint(&data, index, part.next)?;
                        saved = part.next;
                        last_checkpoint = Instant::now();
                    }
                }
                Ok(())
            })();
            if part.next > saved {
                job.checkpoint(&data, index, part.next)?;
            }
            if part.next - started >= PROGRESS_RESET {
                attempt = 1;
            }
            match result {
                Err(error) if error.is::<Retryable>() && attempt < ATTEMPTS => {
                    self.retry_delay(attempt)?
                }
                result => return result,
            }
        }
    }

    fn single(
        &self,
        agent: &ureq::Agent,
        url: &str,
        job: &Job,
        mut response: Option<Box<ureq::Response>>,
        allow_html: bool,
    ) -> Result<()> {
        for attempt in 1..=ATTEMPTS {
            self.check()?;
            let result = (|| -> Result<()> {
                let response = match response.take() {
                    Some(response) => *response,
                    None => http::get(agent, url)?,
                };
                let total = http::full_length(&response)?;
                http::check_content_type(&response, allow_html)?;
                job.prepare_single()?;
                self.update(0, total, "Downloading · 1 connection · restart only");
                self.copy_full(response.into_reader(), job.data()?, total)
            })();
            match result {
                Err(error) if error.is::<Retryable>() && attempt < ATTEMPTS => {
                    self.retry_delay(attempt)?
                }
                result => return result,
            }
        }
        unreachable!()
    }

    fn copy_full(
        &self,
        mut input: impl Read,
        mut output: File,
        expected: Option<u64>,
    ) -> Result<()> {
        let mut buffer = vec![0; BLOCK];
        let mut bytes = 0;
        loop {
            self.check()?;
            let length = read_network(&mut input, &mut buffer)?;
            self.check()?;
            if length == 0 {
                break;
            }
            bytes += length as u64;
            if expected.is_some_and(|total| bytes > total) {
                bail!("The response was larger than its advertised size.");
            }
            output
                .write_all(&buffer[..length])
                .context("Could not write partial data")?;
            self.add(length, None);
        }
        if expected.is_some_and(|total| bytes != total) {
            return Err(Retryable("The server ended the download early.".into()).into());
        }
        output.sync_all().context("Could not flush downloaded data")
    }
}

fn read_network(input: &mut impl Read, buffer: &mut [u8]) -> Result<usize> {
    loop {
        match input.read(buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(
                    Retryable(format!("The connection stopped ({:?}).", error.kind())).into(),
                );
            }
            Ok(length) => return Ok(length),
        }
    }
}

pub fn validate_url(text: &str) -> Result<url::Url> {
    let text = text.trim();
    let mut url = url::Url::parse(text).context("Enter a direct HTTP or HTTPS URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
            && !text.chars().any(char::is_whitespace),
        "Enter a direct HTTP or HTTPS URL without embedded credentials or whitespace."
    );
    url.set_fragment(None);
    Ok(url)
}

pub fn open_partial(path: &Path) -> Result<(String, std::path::PathBuf, usize)> {
    storage::open_partial(path)
}

pub fn resume_manifest(path: &Path) -> Option<std::path::PathBuf> {
    storage::resume_manifest(path)
}

pub fn saved_progress(destination: &Path, url: &str) -> Option<Progress> {
    storage::saved_progress(destination, url).ok().flatten()
}

pub fn discard_partial(destination: &Path, url: &str) -> Result<()> {
    storage::discard_partial(destination, validate_url(url)?.as_str())
}

pub fn partial_occupies(destination: &Path) -> Result<bool> {
    storage::partial_occupies(destination)
}
