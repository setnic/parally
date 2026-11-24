use parallel_downloader::{
    BrowserHeaders, Cancelled, Download, RestartRequired, StartMode, discard_partial,
    partial_occupies, saved_progress,
};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    },
    thread,
    time::{Duration, Instant},
};

const TOTAL: usize = 768 * 1024 + 3;
const CUT: usize = 12 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Range = 0,
    Full,
    Interrupted,
    NewVersion,
    InvalidRange,
    Slow,
    WeakTag,
    OncePerPart,
    InterruptedFull,
    SlowProbe,
    Html,
    HtmlWrongMime,
}

#[derive(Debug)]
struct Request {
    range: Option<(usize, usize)>,
    validator: Option<String>,
    browser: Vec<String>,
}

struct Server {
    url: String,
    mode: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Request>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn new(mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/payload", listener.local_addr().unwrap());
        let mut server = Self {
            url,
            mode: Arc::new(AtomicUsize::new(mode as usize)),
            stopped: Arc::new(AtomicBool::new(false)),
            requests: Arc::new(Mutex::new(Vec::new())),
            thread: None,
        };
        let mode = server.mode.clone();
        let stopped = server.stopped.clone();
        let requests = server.requests.clone();
        server.thread = Some(thread::spawn(move || {
            let original = Arc::new(payload(0));
            let changed = Arc::new(payload(37));
            while !stopped.load(Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let mode = mode.load(Relaxed);
                        let data = if mode == Mode::NewVersion as usize {
                            changed.clone()
                        } else {
                            original.clone()
                        };
                        let requests = requests.clone();
                        thread::spawn(move || serve(stream, mode, &data, &requests));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(3));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        }));
        server
    }

    fn mode(&self, mode: Mode) {
        self.mode.store(mode as usize, Relaxed);
        self.requests.lock().unwrap().clear();
    }

    fn segments(&self) -> Vec<(usize, usize)> {
        let mut ranges: Vec<_> = self
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter_map(|request| {
                request.validator.as_ref()?;
                request.range
            })
            .collect();
        ranges.sort_unstable();
        ranges
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopped.store(true, Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn serve(mut stream: TcpStream, mode: usize, data: &[u8], requests: &Mutex<Vec<Request>>) {
    // Accepted sockets can inherit the listener's nonblocking flag on BSD.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    if !reader.read_line(&mut line).is_ok_and(|bytes| bytes > 0)
        || line != "GET /payload HTTP/1.1\r\n"
    {
        return;
    }
    line.clear();
    let mut range = None;
    let mut validator = None;
    let mut browser = Vec::new();
    loop {
        if !reader.read_line(&mut line).is_ok_and(|bytes| bytes > 0) {
            return;
        }
        if line == "\r\n" {
            break;
        }
        let Some((header, value)) = line.split_once(':') else {
            return;
        };
        if header.eq_ignore_ascii_case("range")
            && let Some(value) = value.trim().strip_prefix("bytes=")
        {
            let (start, end) = value.trim().split_once('-').unwrap();
            range = Some((
                start.parse::<usize>().unwrap(),
                end.parse::<usize>().unwrap(),
            ));
        }
        if header.eq_ignore_ascii_case("if-range") {
            validator = Some(value.trim().to_owned());
        }
        if header.eq_ignore_ascii_case("cookie") || header.eq_ignore_ascii_case("referer") {
            browser.push(format!("{header}: {}", value.trim()));
        }
        line.clear();
    }
    let tag = match mode {
        value if value == Mode::NewVersion as usize => "\"v2\"",
        value if value == Mode::WeakTag as usize => "W/\"v1\"",
        _ => "\"v1\"",
    };
    let matches = validator.as_deref().is_none_or(|value| value == tag);
    let mut requests = requests.lock().unwrap();
    let first = !requests.iter().any(|request| request.range == range);
    requests.push(Request {
        range,
        validator,
        browser,
    });
    drop(requests);

    let write = |stream: &mut TcpStream, bytes: &[u8]| stream.write_all(bytes).is_ok();
    if mode == Mode::Html as usize || mode == Mode::HtmlWrongMime as usize {
        let body = b"\xef\xbb\xbf <!DOCTYPE html><html><body>Sign in</body></html>";
        let mime = if mode == Mode::Html as usize {
            "text/html; charset=UTF-8"
        } else {
            "application/octet-stream"
        };
        let header = format!(
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: {mime}\r\n\r\n",
            body.len()
        );
        if write(&mut stream, header.as_bytes()) {
            write(&mut stream, body);
        }
        return;
    }
    if let Some((start, end)) = range.filter(|_| {
        mode != Mode::Full as usize && mode != Mode::InterruptedFull as usize && matches
    }) {
        if mode == Mode::SlowProbe as usize && start == end {
            thread::sleep(Duration::from_millis(250));
        }
        let actual_start = if mode == Mode::InvalidRange as usize && start != end {
            start + 1
        } else {
            start
        };
        let header = format!(
            "HTTP/1.1 206 Partial Content\r\nConnection: close\r\nETag: {tag}\r\nContent-Length: {}\r\nContent-Range: bytes {actual_start}-{end}/{TOTAL}\r\n\r\n",
            end - start + 1,
        );
        if !write(&mut stream, header.as_bytes()) {
            return;
        }
        let bytes = &data[start..=end];
        if start != end
            && (mode == Mode::Interrupted as usize
                || mode == Mode::OncePerPart as usize
                    && first
                    && end - start + 1 > CUT * 2
                    && initial_ranges(3).contains(&(start, end)))
        {
            write(&mut stream, &bytes[..CUT]);
            return;
        }
        if (mode == Mode::Slow as usize || mode == Mode::SlowProbe as usize) && start != end {
            for chunk in bytes.chunks(4096) {
                if !write(&mut stream, chunk) {
                    return;
                }
                thread::sleep(Duration::from_millis(9));
            }
            return;
        }
        write(&mut stream, bytes);
    } else if write(
        &mut stream,
        format!(
            "HTTP/1.1 200 OK\r\nConnection: close\r\nETag: {tag}\r\nContent-Length: {TOTAL}\r\n\r\n"
        )
        .as_bytes(),
    ) {
        if mode == Mode::InterruptedFull as usize {
            write(&mut stream, &data[..CUT]);
        } else {
            write(&mut stream, data);
        }
    }
}

#[test]
fn web_pages_are_never_published_under_a_non_html_filename() {
    for mode in [Mode::Html, Mode::HtmlWrongMime] {
        let folder = Folder::new();
        let server = Server::new(mode);
        let path = folder.destination().with_extension("gguf");
        let error = Download::default().run(&server.url, &path, 4).unwrap_err();
        assert!(
            error.to_string().contains("direct file download link"),
            "{error:#}"
        );
        assert!(!path.exists());
        let html = path.with_extension("html");
        Download::default().run(&server.url, &html, 4).unwrap();
        assert!(
            fs::read_to_string(html)
                .unwrap()
                .contains("<!DOCTYPE html>")
        );
    }
}

#[test]
fn active_connections_can_increase_and_decrease_without_restarting() {
    let folder = Folder::new();
    let server = Server::new(Mode::Slow);
    let path = folder.destination();
    let job = Download::default();
    thread::scope(|scope| {
        let worker = scope.spawn(|| job.run(&server.url, &path, 1));
        let deadline = Instant::now() + Duration::from_secs(8);
        let wait = |predicate: &dyn Fn() -> bool| {
            while !predicate() {
                assert!(Instant::now() < deadline && !worker.is_finished());
                thread::sleep(Duration::from_millis(5));
            }
        };
        wait(&|| job.progress().bytes >= 8192);
        let saved = job.progress().bytes;
        job.set_connections(4);
        wait(&|| job.progress().status.contains("4 active"));
        assert!(job.progress().bytes >= saved);
        wait(&|| server.segments().len() >= 5);
        job.set_connections(2);
        wait(&|| job.progress().status.contains("2 active"));
        assert!(job.progress().bytes >= saved);
        worker.join().unwrap().unwrap();
    });
    assert_finished(&folder, 0);
    assert_eq!(
        server
            .segments()
            .iter()
            .filter(|(start, _)| *start == 0)
            .count(),
        1
    );
}

#[test]
fn pause_then_resume_from_unbalanced_range_offsets() {
    let folder = Folder::new();
    let server = Server::new(Mode::Slow);
    let path = folder.destination();
    let pause = Download::default();
    thread::scope(|scope| {
        let worker = scope.spawn(|| pause.run(&server.url, &path, 4));
        let start = Instant::now();
        while pause
            .progress()
            .segments
            .iter()
            .filter(|item| item.bytes > 0)
            .count()
            != 4
        {
            assert!(start.elapsed() < Duration::from_secs(5) && !worker.is_finished());
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            discard_partial(&path, &server.url).is_err(),
            "A running worker must hold the partial lock"
        );
        assert!(pause.cancel());
        assert!(worker.join().unwrap().unwrap_err().is::<Cancelled>());
    });
    let saved = saved_progress(&path, &server.url).unwrap();
    assert_eq!(saved.segments.len(), 4);
    assert_ne!(saved.segments[0].length, saved.segments[3].length);
    assert!(
        saved
            .segments
            .iter()
            .all(|item| item.bytes > 0 && item.bytes < item.length)
    );
    server.mode(Mode::SlowProbe);
    let resume = Download::with_mode(StartMode::Resume);
    thread::scope(|scope| {
        let worker = scope.spawn(|| resume.run(&server.url, &path, 4));
        let start = Instant::now();
        loop {
            let progress = resume.progress();
            if progress.status.starts_with("Checking") {
                assert_eq!(progress.bytes, saved.bytes);
                assert_eq!(progress.segments, saved.segments);
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(3) && !worker.is_finished());
            thread::sleep(Duration::from_millis(4));
        }
        worker.join().unwrap().unwrap();
    });
    let mut start = 0;
    let expected = saved
        .segments
        .iter()
        .map(|item| {
            let range = (
                start + item.bytes as usize,
                start + item.length as usize - 1,
            );
            start += item.length as usize;
            range
        })
        .collect::<Vec<_>>();
    assert_eq!(server.segments(), expected);
    assert_finished(&folder, 0);
}

#[test]
fn incompatible_resume_keeps_partial_until_explicit_restart() {
    let folder = Folder::new();
    let server = Server::new(Mode::Interrupted);
    let path = folder.destination();
    assert!(Download::default().run(&server.url, &path, 3).is_err());
    let saved = fs::read(folder.manifest()).unwrap();
    assert!(saved_progress(&path, &server.url).is_some());
    for mode in [Mode::Full, Mode::NewVersion] {
        server.mode(mode);
        let failure = Download::with_mode(StartMode::Resume)
            .run(&server.url, &path, 3)
            .unwrap_err();
        assert!(failure.is::<RestartRequired>(), "{failure:#}");
        assert_eq!(fs::read(folder.manifest()).unwrap(), saved);
        assert!(server.segments().is_empty());
        assert!(!path.exists());
    }
    Download::with_mode(StartMode::Restart)
        .run(&server.url, &path, 3)
        .unwrap();
    assert_eq!(server.segments(), initial_ranges(3));
    assert_finished(&folder, 37);
}

#[test]
fn discard_preserves_other_jobs_and_existing_finished_files() {
    let folder = Folder::new();
    let server = Server::new(Mode::Slow);
    let path = folder.destination();
    let job = Download::default();
    thread::scope(|scope| {
        let worker = scope.spawn(|| job.run(&server.url, &path, 3));
        let deadline = Instant::now() + Duration::from_secs(5);
        while job.progress().bytes < 30_000 && !worker.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(job.progress().bytes >= 30_000);
        assert!(discard_partial(&path, &server.url).is_err());
        assert!(job.cancel());
        assert!(worker.join().unwrap().unwrap_err().is::<Cancelled>());
    });
    assert!(folder.manifest().exists());
    let other = Server::new(Mode::Interrupted);
    assert!(discard_partial(&path, &other.url).is_err());
    let unrelated = folder.manifest().with_file_name("notes.txt");
    fs::write(&unrelated, b"keep notes").unwrap();
    fs::write(&path, b"keep destination").unwrap();
    discard_partial(&path, &server.url).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"keep destination");
    assert_eq!(fs::read(&unrelated).unwrap(), b"keep notes");
    assert!(!folder.manifest().exists());
    assert!(!folder.manifest().with_file_name("data.part").exists());
    fs::remove_file(unrelated).unwrap();
    assert!(!partial_occupies(&path).unwrap());
}

fn payload(change: usize) -> Vec<u8> {
    (0..TOTAL)
        .map(|index| (index ^ (index >> 9) ^ (index >> 17) ^ change) as u8)
        .collect()
}

fn initial_ranges(count: usize) -> Vec<(usize, usize)> {
    assert_eq!(TOTAL % count, 0);
    let part = TOTAL / count;
    (0..count)
        .map(|index| (index * part, (index + 1) * part - 1))
        .collect()
}

struct Folder(PathBuf);

impl Folder {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let sequence = NEXT.fetch_add(1, Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "parallel-downloader-test-{}-{}-{sequence}",
            std::process::id(),
            now.as_nanos(),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn destination(&self) -> PathBuf {
        self.0.join("result.bin")
    }

    fn manifest(&self) -> PathBuf {
        self.0
            .join(".result.bin.parallel-download")
            .join("resume.json")
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn assert_finished(folder: &Folder, change: usize) {
    assert_eq!(fs::read(folder.destination()).unwrap(), payload(change));
    assert!(!folder.manifest().parent().unwrap().exists());
}

// A separate process ensures no in-memory offsets or HTTP connections survive a restart.
#[test]
fn child_download() {
    let Ok(url) = std::env::var("PARALLEL_DOWNLOADER_TEST_URL") else {
        return;
    };
    let destination = PathBuf::from(std::env::var_os("PARALLEL_DOWNLOADER_TEST_DEST").unwrap());
    let download = Download::default();
    let result = download.run(&url, &destination, 3);
    if std::env::var_os("PARALLEL_DOWNLOADER_TEST_INTERRUPTED").is_some() {
        let error = result.expect_err("the connection should break repeatedly");
        assert!(
            error.to_string().contains("connection stopped")
                || error.to_string().contains("ended a range response early"),
            "{error:#}"
        );
        assert!(!destination.exists());
    } else {
        result.unwrap();
    }
}

fn child(server: &Server, url: &str, path: &Path, interrupted: bool) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "child_download", "--nocapture"])
        .env("PARALLEL_DOWNLOADER_TEST_URL", url)
        .env("PARALLEL_DOWNLOADER_TEST_DEST", path);
    if interrupted {
        command.env("PARALLEL_DOWNLOADER_TEST_INTERRUPTED", "1");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}\nRequests: {:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        server.requests.lock().unwrap(),
    );
}

#[test]
fn interrupted_ranges_resume_in_another_process() {
    let folder = Folder::new();
    let server = Server::new(Mode::Interrupted);
    child(&server, &server.url, &folder.destination(), true);
    assert_eq!(server.segments().len(), 12);
    let (url, destination) = (server.url.clone(), folder.destination());
    server.mode(Mode::Range);
    child(&server, &url, &destination, false);
    assert_eq!(
        server.segments(),
        initial_ranges(3)
            .into_iter()
            .map(|(start, end)| (start + 4 * CUT, end))
            .collect::<Vec<_>>()
    );
    assert_finished(&folder, 0);
}

#[test]
fn retry_resumes_without_restarting_the_process() {
    let folder = Folder::new();
    let server = Server::new(Mode::OncePerPart);
    Download::default()
        .run(&server.url, &folder.destination(), 3)
        .unwrap();
    let base = initial_ranges(3);
    let mut expected = base.clone();
    expected.extend(base.into_iter().map(|(start, end)| (start + CUT, end)));
    expected.sort_unstable();
    assert_eq!(server.segments(), expected);
    assert_finished(&folder, 0);
}

#[test]
fn new_etag_discards_old_ranges() {
    let folder = Folder::new();
    let server = Server::new(Mode::Interrupted);
    assert!(
        Download::default()
            .run(&server.url, &folder.destination(), 3)
            .is_err()
    );
    server.mode(Mode::NewVersion);
    Download::default()
        .run(&server.url, &folder.destination(), 3)
        .unwrap();
    assert_eq!(server.segments(), initial_ranges(3));
    assert_finished(&folder, 37);
}

#[test]
fn falls_back_for_no_range_bad_ranges_and_weak_tags() {
    for mode in [Mode::Full, Mode::InvalidRange, Mode::WeakTag] {
        let folder = Folder::new();
        let server = Server::new(mode);
        Download::default()
            .run(&server.url, &folder.destination(), 3)
            .unwrap_or_else(|error| {
                panic!(
                    "{mode:?}: {error:#}; requests: {:?}",
                    server.requests.lock().unwrap()
                )
            });
        if mode != Mode::InvalidRange {
            assert!(server.segments().is_empty());
        }
        assert_finished(&folder, 0);
    }
}

#[test]
fn failed_full_transfer_starts_over_safely() {
    let folder = Folder::new();
    let server = Server::new(Mode::InterruptedFull);
    assert!(
        Download::default()
            .run(&server.url, &folder.destination(), 3)
            .is_err()
    );
    assert!(!folder.destination().exists());
    server.mode(Mode::Full);
    Download::default()
        .run(&server.url, &folder.destination(), 3)
        .unwrap();
    assert_finished(&folder, 0);
}

#[test]
fn pause_preserves_partial_and_refuses_concurrent_download() {
    let folder = Folder::new();
    let server = Server::new(Mode::Slow);
    let job = Arc::new(Download::default());
    let destination = folder.destination();
    let worker = {
        let job = job.clone();
        let url = server.url.clone();
        let destination = destination.clone();
        thread::spawn(move || job.run(&url, &destination, 3))
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while job.progress().bytes < 30_000 && !worker.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(job.progress().bytes >= 30_000);
    let collision = Download::default()
        .run(&server.url, &destination, 3)
        .unwrap_err();
    assert!(collision.to_string().contains("already using"));
    assert!(job.cancel());
    assert!(worker.join().unwrap().unwrap_err().is::<Cancelled>());
    assert!(!destination.exists());
    assert!(folder.manifest().exists());
    server.mode(Mode::Range);
    Download::default()
        .run(&server.url, &destination, 2)
        .unwrap();
    assert!(server.segments().iter().any(|(start, end)| {
        initial_ranges(3)
            .iter()
            .any(|(first, last)| start > first && end == last)
    }));
    assert_finished(&folder, 0);
}

#[test]
fn never_overwrites_a_destination_created_during_a_download() {
    let folder = Folder::new();
    let server = Server::new(Mode::Slow);
    let destination = folder.destination();
    let job = Download::default();
    thread::scope(|scope| {
        let worker = scope.spawn(|| job.run(&server.url, &destination, 1));
        let deadline = Instant::now() + Duration::from_secs(5);
        while job.progress().bytes == 0 && !worker.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(job.progress().bytes > 0);
        fs::write(&destination, b"keep me").unwrap();
        assert!(worker.join().unwrap().is_err());
    });
    assert_eq!(fs::read(&destination).unwrap(), b"keep me");
    assert!(folder.manifest().exists());
}

#[test]
fn two_downloads_run_together_and_browser_headers_reach_every_request() {
    let folder = Folder::new();
    let first = Server::new(Mode::Slow);
    let second = Server::new(Mode::Slow);
    let browser = Download::default().with_headers(BrowserHeaders {
        cookie: "session=abc".into(),
        referrer: "https://example.test/page".into(),
    });
    let ordinary = Download::default();
    let normal_path = folder.0.join("normal.bin");
    let browser_path = folder.destination();
    thread::scope(|scope| {
        let left = scope.spawn(|| browser.run(&first.url, &browser_path, 3));
        let right = scope.spawn(|| ordinary.run(&second.url, &normal_path, 3));
        let until = Instant::now() + Duration::from_secs(4);
        while (browser.progress().bytes == 0 || ordinary.progress().bytes == 0)
            && !left.is_finished()
            && !right.is_finished()
            && Instant::now() < until
        {
            thread::sleep(Duration::from_millis(4));
        }
        assert!(browser.progress().bytes > 0 && ordinary.progress().bytes > 0);
        assert!(!left.is_finished() && !right.is_finished());
        left.join().unwrap().unwrap();
        right.join().unwrap().unwrap();
    });
    assert_finished(&folder, 0);
    assert_eq!(fs::read(normal_path).unwrap(), payload(0));
    let sent = first.requests.lock().unwrap();
    assert!(sent.len() > 1);
    for request in sent.iter() {
        assert_eq!(
            request.browser,
            ["Cookie: session=abc", "Referer: https://example.test/page"]
        );
    }
    assert!(
        second
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.browser.is_empty())
    );
}
