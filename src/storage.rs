use crate::{
    MAX_STREAMS, Progress, RestartRequired, SegmentProgress, StartMode, http::strong_etag,
    validate_url,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

const MANIFEST: &str = "resume.json";
const DATA: &str = "data.part";
const LOCK: &str = "download.lock";
const TEMP_METADATA: &str = "resume.tmp";
const SUFFIX: &str = ".parallel-download";
const VERSION: u32 = 1;
const MAX_PARTS: usize = 256;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Remote {
    pub url: String,
    pub etag: String,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Part {
    pub start: u64,
    pub end: u64,
    pub next: u64,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Manifest {
    version: u32,
    url: String,
    name: String,
    streams: usize,
    pub remote: Option<Remote>,
    pub parts: Vec<Part>,
}

impl Manifest {
    pub fn progress(&self, total: u64) -> Progress {
        let segments: Vec<_> = self
            .parts
            .iter()
            .map(|part| SegmentProgress {
                length: part.end - part.start,
                bytes: part.next - part.start,
            })
            .collect();
        Progress {
            bytes: segments.iter().map(|part| part.bytes).sum(),
            total: Some(total),
            status: String::new(),
            segments,
        }
    }

    fn read(path: &Path) -> Result<Self> {
        let data = metadata(path)?.context("The resume file is missing")?;
        ensure!(
            data.is_file() && data.len() <= 128 * 1024,
            "Invalid resume file."
        );
        let manifest: Self =
            serde_json::from_reader(File::open(path)?).context("Invalid resume file")?;
        ensure!(
            manifest.version == VERSION,
            "Unsupported resume file version."
        );
        validate_url(&manifest.url)?;
        ensure!(
            valid_name(&manifest.name) && (1..=MAX_STREAMS).contains(&manifest.streams),
            "Invalid destination or connection count in the resume file."
        );
        if let Some(remote) = &manifest.remote {
            validate_url(&remote.url)?;
            ensure!(
                strong_etag(&remote.etag)
                    && remote.size > 0
                    && (1..=MAX_PARTS).contains(&manifest.parts.len())
                    && manifest.parts.len() as u64 <= remote.size,
                "Invalid range plan."
            );
            let mut end = 0;
            for actual in &manifest.parts {
                ensure!(
                    actual.start == end
                        && actual.start < actual.end
                        && (actual.start..=actual.end).contains(&actual.next),
                    "Invalid saved byte range."
                );
                end = actual.end;
            }
            ensure!(end == remote.size, "Invalid saved byte range.");
        } else {
            ensure!(manifest.parts.is_empty(), "Invalid restart state.");
        }
        Ok(manifest)
    }
}

pub struct Job {
    dir: PathBuf,
    lock: File,
    manifest: Mutex<Manifest>,
}

impl Job {
    pub fn open(destination: &Path, url: &str, streams: usize, mode: StartMode) -> Result<Self> {
        ensure!(
            destination.is_absolute(),
            "Choose an absolute destination path."
        );
        let name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| valid_name(name))
            .context("Choose a valid destination filename")?;
        let parent = destination
            .parent()
            .context("Choose a destination folder")?
            .canonicalize()
            .context("The destination folder is unavailable")?;
        let destination = parent.join(name);
        ensure!(
            metadata(&destination)?.is_none(),
            "The destination file already exists."
        );
        let dir = parent.join(sidecar_name(name));
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                ensure!(
                    metadata(&dir)?.is_some_and(|item| item.is_dir()),
                    "The resume folder path is already in use."
                );
            }
            Err(error) => return Err(error).context("Could not create the resume folder"),
        }

        let lock = lock_job(&dir)?;
        ensure!(
            metadata(&destination)?.is_none(),
            "The destination file already exists."
        );
        let manifest_path = dir.join(MANIFEST);
        let existing = match metadata(&manifest_path)? {
            Some(_) => Some(Manifest::read(&manifest_path)?),
            None => {
                for entry in fs::read_dir(&dir)? {
                    let name = entry?.file_name();
                    ensure!(
                        name == LOCK || name == TEMP_METADATA,
                        "The resume folder contains unrelated files."
                    );
                }
                None
            }
        };
        file_or_missing(&dir.join(DATA))?;
        file_or_missing(&dir.join(TEMP_METADATA))?;
        let old_url = existing.as_ref().map(|old| old.url.clone());
        if mode == StartMode::Resume {
            ensure!(
                old_url.as_deref() == Some(url),
                RestartRequired("The saved download is missing or belongs to another link.")
            );
        }
        let mut manifest = existing.unwrap_or_else(|| Manifest {
            version: VERSION,
            url: url.to_owned(),
            name: name.to_owned(),
            streams,
            remote: None,
            parts: Vec::new(),
        });
        ensure!(
            manifest.name.eq_ignore_ascii_case(name) || manifest.name == name,
            "This resume file belongs to a different destination."
        );
        manifest.url = url.to_owned();
        manifest.streams = streams;
        let job = Self {
            dir,
            lock,
            manifest: Mutex::new(manifest),
        };
        if old_url.as_deref() != Some(url) {
            job.save(&job.manifest.lock().unwrap())?;
        }
        Ok(job)
    }

    pub fn data(&self) -> Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.dir.join(DATA))
            .context("Could not open the partial file")
    }

    pub fn checkpoint_progress(&self) -> Result<Progress> {
        let manifest = self.manifest.lock().unwrap();
        let Some(remote) = &manifest.remote else {
            return Err(RestartRequired("No verified byte ranges are saved.").into());
        };
        let progress = manifest.progress(remote.size);
        ensure!(
            progress.bytes > 0
                && metadata(&self.dir.join(DATA))?
                    .is_some_and(|item| item.is_file() && item.len() == remote.size),
            RestartRequired("The partial data is missing or its length no longer matches.")
        );
        Ok(progress)
    }

    pub fn prepare_ranges(
        &self,
        remote: Remote,
        streams: usize,
        mode: StartMode,
    ) -> Result<Manifest> {
        let mut manifest = self.manifest.lock().unwrap();
        if mode != StartMode::Restart
            && manifest.remote.as_ref() == Some(&remote)
            && metadata(&self.dir.join(DATA))?.is_some_and(|item| item.len() == remote.size)
        {
            return Ok(manifest.clone());
        }
        if mode == StartMode::Resume {
            let reason = if manifest.remote.as_ref() != Some(&remote) {
                "The server reports a different file version, size, or final URL."
            } else {
                "The partial data is missing or its length no longer matches."
            };
            return Err(RestartRequired(reason).into());
        }
        self.reset(&mut manifest, remote.size)?;
        manifest.parts = parts(
            remote.size,
            streams.min(remote.size.try_into().unwrap_or(usize::MAX)),
        );
        manifest.remote = Some(remote);
        self.save(&manifest)?;
        Ok(manifest.clone())
    }

    pub fn prepare_single(&self) -> Result<()> {
        self.reset(&mut self.manifest.lock().unwrap(), 0)
    }

    pub fn balance(&self, streams: usize) -> Result<Manifest> {
        let mut manifest = self.manifest.lock().unwrap();
        let mut pending = manifest
            .parts
            .iter()
            .filter(|part| part.next < part.end)
            .count();
        let mut changed = false;
        while pending < streams && manifest.parts.len() < MAX_PARTS {
            let Some((index, part)) = manifest
                .parts
                .iter_mut()
                .enumerate()
                .filter(|(_, part)| part.end - part.next >= 2)
                .max_by_key(|(_, part)| part.end - part.next)
            else {
                break;
            };
            let split = part.next + (part.end - part.next) / 2;
            let tail = Part {
                start: split,
                end: part.end,
                next: split,
            };
            part.end = split;
            manifest.parts.insert(index + 1, tail);
            changed = true;
            pending += 1;
        }
        if changed {
            self.save(&manifest)?;
        }
        Ok(manifest.clone())
    }

    fn reset(&self, manifest: &mut Manifest, length: u64) -> Result<()> {
        // Invalidate old offsets durably before touching the bytes they describe.
        manifest.remote = None;
        manifest.parts.clear();
        self.save(manifest)?;
        let data =
            File::create(self.dir.join(DATA)).context("Could not create the partial file")?;
        data.set_len(length)
            .context("Could not reserve space for the download")?;
        data.sync_all()
            .context("Could not flush the partial file")?;
        Ok(())
    }

    pub fn checkpoint(&self, data: &File, index: usize, next: u64) -> Result<()> {
        data.sync_data().context("Could not flush partial data")?;
        let mut manifest = self.manifest.lock().unwrap();
        manifest.parts[index].next = next;
        self.save(&manifest)
    }

    fn save(&self, manifest: &Manifest) -> Result<()> {
        let temp = self.dir.join(TEMP_METADATA);
        let mut file = File::create(&temp).context("Could not write the resume file")?;
        serde_json::to_writer(&mut file, manifest)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        move_file(&temp, &self.dir.join(MANIFEST), true).context("Could not commit the resume file")
    }

    pub fn publish(&self, destination: &Path) -> Result<()> {
        let manifest = self.manifest.lock().unwrap();
        let data = self.dir.join(DATA);
        if let Some(remote) = &manifest.remote {
            ensure!(
                manifest.parts.iter().all(|part| part.next == part.end)
                    && fs::metadata(&data)?.len() == remote.size,
                "The download is incomplete."
            );
        }
        let destination = self
            .dir
            .parent()
            .unwrap()
            .join(destination.file_name().unwrap());
        move_file(&data, &destination, false).context("Could not save the finished file")
    }

    pub fn cleanup(self) {
        // On Unix publish leaves data.part as a second hard link; on Windows it was already moved.
        for file in [DATA, MANIFEST, TEMP_METADATA] {
            let _ = fs::remove_file(self.dir.join(file));
        }
        drop(self.lock);
        let _ = fs::remove_file(self.dir.join(LOCK));
        let _ = fs::remove_dir(&self.dir);
    }
}

pub fn resume_manifest(destination: &Path) -> Option<PathBuf> {
    let name = destination
        .file_name()?
        .to_str()
        .filter(|name| valid_name(name))?;
    Some(
        destination
            .parent()?
            .join(sidecar_name(name))
            .join(MANIFEST),
    )
}

pub fn saved_progress(destination: &Path, url: &str) -> Result<Option<Progress>> {
    let path = resume_manifest(destination).context("Invalid destination")?;
    let manifest = Manifest::read(&path)?;
    let Some(remote) = &manifest.remote else {
        return Ok(None);
    };
    let data = path.with_file_name(DATA);
    if manifest.url != url
        || !metadata(&data)?.is_some_and(|item| item.is_file() && item.len() == remote.size)
    {
        return Ok(None);
    }
    let progress = manifest.progress(remote.size);
    Ok((progress.bytes > 0).then_some(progress))
}

fn lock_job(dir: &Path) -> Result<File> {
    let path = dir.join(LOCK);
    file_or_missing(&path)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .context("Could not open the download lock")?;
    lock.try_lock()
        .context("Another download is already using this destination")?;
    Ok(lock)
}

pub fn partial_occupies(destination: &Path) -> Result<bool> {
    let manifest = resume_manifest(destination).context("Invalid destination")?;
    let dir = manifest.parent().unwrap();
    let Some(meta) = metadata(dir)? else {
        return Ok(false);
    };
    if !meta.is_dir() {
        return Ok(true);
    }
    file_or_missing(&dir.join(LOCK))?;
    for item in fs::read_dir(dir)? {
        if item?.file_name() != LOCK {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn discard_partial(destination: &Path, url: &str) -> Result<()> {
    ensure!(destination.is_absolute(), "Invalid destination.");
    let path = resume_manifest(destination).context("Invalid destination")?;
    let dir = path.parent().unwrap();
    let Some(meta) = metadata(dir)? else {
        return Ok(());
    };
    ensure!(meta.is_dir(), "The partial download path is not a folder.");
    let _lock = lock_job(dir)?;
    if metadata(&path)?.is_none() {
        ensure!(
            metadata(&dir.join(DATA))?.is_none() && metadata(&dir.join(TEMP_METADATA))?.is_none(),
            "Could not confirm who owns the partial files."
        );
        return Ok(());
    }
    let manifest = Manifest::read(&path)?;
    let name = destination.file_name().unwrap().to_string_lossy();
    ensure!(
        manifest.url == url && manifest.name.eq_ignore_ascii_case(&name),
        "The partial files belong to a different download."
    );
    for file in [DATA, TEMP_METADATA, MANIFEST] {
        file_or_missing(&dir.join(file))?;
    }
    // Leave the lock in place so another process cannot acquire a different lock for this folder.
    for file in [DATA, TEMP_METADATA, MANIFEST] {
        match fs::remove_file(dir.join(file)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Could not remove the partial download"),
        }
    }
    Ok(())
}

fn parts(size: u64, count: usize) -> Vec<Part> {
    let width = size / count as u64;
    let extra = size % count as u64;
    let mut start = 0;
    (0..count)
        .map(|index| {
            let end = start + width + u64::from((index as u64) < extra);
            let part = Part {
                start,
                end,
                next: start,
            };
            start = end;
            part
        })
        .collect()
}

fn valid_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    !(name.is_empty()
        || name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        || (stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })))
}

fn sidecar_name(name: &str) -> String {
    format!(".{name}{SUFFIX}")
}

fn metadata(path: &Path) -> Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(item) if linked(&item) => {
            bail!("A download or resume path is a link or reparse point.");
        }
        Ok(item) => Ok(Some(item)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Could not check a download or resume path"),
    }
}

fn file_or_missing(path: &Path) -> Result<()> {
    ensure!(
        metadata(path)?.is_none_or(|item| item.is_file()),
        "A download path is not a file."
    );
    Ok(())
}

fn linked(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    return metadata.file_attributes() & 0x400 != 0;
    #[cfg(not(windows))]
    return metadata.file_type().is_symlink();
}

#[cfg(windows)]
fn move_file(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let (from, to) = (wide(from), wide(to));
    // Omitting REPLACE_EXISTING for publication gives an atomic no-overwrite move.
    let flags = if replace {
        MOVEFILE_REPLACE_EXISTING
    } else {
        0
    };
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags | MOVEFILE_WRITE_THROUGH) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn move_file(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    if replace {
        return fs::rename(from, to);
    }
    // A hard link in the same directory tree publishes atomically without overwrite.
    // Job::cleanup removes the private link after successful publication.
    match fs::hard_link(from, to) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            // shortcut: exFAT/FAT and some network shares lack hard links, so check-then-rename
            // can race with another writer; use renameat2/renamex_np if that ever matters.
            if fs::symlink_metadata(to).is_ok() {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            fs::rename(from, to)
        }
        result => result,
    }
}
