use anyhow::{Context, Result, ensure};
use parallel_downloader::{
    BrowserHeaders, MAX_STREAMS, Progress, SegmentProgress, partial_occupies, resume_manifest, saved_progress,
    validate_url,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Active,
    Paused,
    Interrupted,
    Complete,
}

#[derive(Deserialize, Serialize)]
pub struct Entry {
    pub id: u64,
    pub url: String,
    pub path: PathBuf,
    pub streams: usize,
    pub state: State,
    pub bytes: u64,
    pub total: Option<u64>,
    pub saved: bool,
    pub detail: String,
    #[serde(default)]
    pub canceled: bool,
    #[serde(default)]
    pub restart_required: bool,
    #[serde(skip)]
    pub segments: Vec<SegmentProgress>,
    /// Browser cookie and Referer: kept for resumes in this session, never written to disk.
    #[serde(skip)]
    pub headers: BrowserHeaders,
}

impl Entry {
    pub fn new(id: u64, url: String, path: PathBuf, streams: usize) -> Self {
        Self {
            id,
            url,
            path,
            streams,
            state: State::Interrupted,
            bytes: 0,
            total: None,
            saved: false,
            detail: String::new(),
            canceled: false,
            restart_required: false,
            segments: Vec::new(),
            headers: BrowserHeaders::default(),
        }
    }

    pub fn read_checkpoint(&mut self) {
        let saved = saved_progress(&self.path, &self.url);
        self.saved = saved.is_some();
        self.segments.clear();
        if let Some(progress) = saved {
            self.observe(progress);
        }
    }

    pub fn observe(&mut self, progress: Progress) {
        if progress.total.is_some()
            || (!progress.status.is_empty() && !progress.status.starts_with("Checking"))
        {
            self.bytes = progress.bytes;
            self.total = progress.total;
            self.segments = progress.segments;
            if self.streams == 1 {
                self.segments.clear();
            }
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Library {
    pub entries: Vec<Entry>,
    pub folders: Vec<PathBuf>,
    pub last_folder: Option<usize>,
    pub scale: Scale,
    pub settings: Settings,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub start_at_login: bool,
    pub transparency: u8,
    pub light: bool,
    pub russian: bool,
    pub connections: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            start_at_login: false,
            transparency: 26,
            light: false,
            russian: false,
            connections: 4,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    #[default]
    Auto,
    Fifty,
    SeventyFive,
    Hundred,
}

impl Scale {
    pub const OPTIONS: [Self; 4] = [Self::Auto, Self::Fifty, Self::SeventyFive, Self::Hundred];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Fifty => "50%",
            Self::SeventyFive => "75%",
            Self::Hundred => "100%",
        }
    }
    pub fn zoom(self, native: f32) -> f32 {
        match self {
            Self::Auto => 1.0,
            Self::Fifty => 0.5 / native,
            Self::SeventyFive => 0.75 / native,
            Self::Hundred => 1.0 / native,
        }
    }
}

impl Library {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join("library.json");
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error).context("Could not open download history"),
        };
        let mut library: Self =
            serde_json::from_reader(file).context("Download history is unreadable")?;
        library.settings.connections = library.settings.connections.clamp(1, MAX_STREAMS);
        library.settings.transparency = library.settings.transparency.min(45);
        for entry in &mut library.entries {
            if entry.state == State::Active {
                entry.state = State::Interrupted;
                entry.detail = "Parally closed before this download finished.".into();
            }
            if entry.state != State::Complete && !entry.canceled {
                entry.read_checkpoint();
            }
        }
        Ok(library)
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        write_json(&root.join("library.json"), self).context("Could not save download history")
    }

    pub fn add_folder(&mut self, folder: PathBuf) -> Result<usize> {
        let folder = canonical_folder(&folder)?;
        let id = match self
            .folders
            .iter()
            .position(|saved| same_path(saved, &folder))
        {
            Some(id) => id,
            None => {
                self.folders.push(folder);
                self.folders.len() - 1
            }
        };
        self.last_folder = Some(id);
        Ok(id)
    }

    pub fn next_id(&self) -> u64 {
        self.entries.iter().map(|entry| entry.id).max().unwrap_or(0) + 1
    }

    pub fn available_path(&self, path: &Path) -> Result<PathBuf> {
        let path = normalize_destination(path)?;
        for index in 0..10_000 {
            let candidate = if index == 0 {
                path.clone()
            } else {
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                let extension = path
                    .extension()
                    .map(|ext| format!(".{}", ext.to_string_lossy()))
                    .unwrap_or_default();
                path.with_file_name(format!("{stem} ({index}){extension}"))
            };
            if !exists(&candidate)?
                && !partial_occupies(&candidate)?
                && !self
                    .entries
                    .iter()
                    .any(|entry| entry.state == State::Active && same_path(&entry.path, &candidate))
            {
                return Ok(candidate);
            }
        }
        anyhow::bail!("Choose another destination filename.")
    }
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    let temporary = path.with_extension(format!("{extension}.new"));
    let mut file = File::create(&temporary)?;
    serde_json::to_writer(&mut file, value)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn root() -> Result<PathBuf> {
    let path = crate::platform::app_data()?;
    fs::create_dir_all(path.join("inbox"))?;
    Ok(path)
}

pub fn try_instance_lock(root: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("instance.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(error).context("Could not lock Parally's download history"),
    }
}

pub fn normalize_destination(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute(),
        "Choose a destination file or enter a full path."
    );
    resume_manifest(path).context("Choose a valid destination filename")?;
    Ok(
        canonical_folder(path.parent().context("Choose a destination folder")?)?
            .join(path.file_name().unwrap()),
    )
}

fn canonical_folder(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute() && path.is_dir(),
        "Choose an existing folder with a full path."
    );
    let full = path.canonicalize().context("Could not open the folder")?;
    #[cfg(not(windows))]
    return Ok(full);
    #[cfg(windows)]
    {
        let text = full.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{unc}")));
        }
        Ok(text
            .strip_prefix(r"\\?\")
            .map_or(full.clone(), PathBuf::from))
    }
}

pub fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(target_os = "linux")]
    return left == right;
    #[cfg(not(target_os = "linux"))]
    {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    }
}

pub fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("Could not check the destination"),
    }
}

pub fn suggested_name(link: &str) -> String {
    validate_url(link)
        .ok()
        .and_then(|url| {
            url.path_segments()?
                .next_back()
                .filter(|part| valid_filename(part))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "download.bin".into())
}

pub fn valid_filename(name: &str) -> bool {
    let path = Path::new(name);
    path.file_name()
        .is_some_and(|leaf| leaf == std::ffi::OsStr::new(name))
        && resume_manifest(path).is_some()
}

pub fn valid_streams(streams: usize) -> bool {
    (1..=MAX_STREAMS).contains(&streams)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_presets_settings_and_collisions() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("parally-library-{}", std::process::id()));
        fs::create_dir(&dir)?;
        let mut library = Library::default();
        assert_eq!(
            library.add_folder(dir.clone())?,
            library.add_folder(dir.clone())?
        );
        fs::write(dir.join("test.zip"), b"keep")?;
        let first = library.available_path(&dir.join("test.zip"))?;
        assert_eq!(first.file_name().unwrap(), "test (1).zip");
        let mut entry = Entry::new(1, "https://example.test/file".into(), first, 4);
        entry.state = State::Active;
        entry.headers.cookie = "session=secret".into();
        library.entries.push(entry);
        assert_eq!(
            library
                .available_path(&dir.join("TEST.ZIP"))?
                .file_name()
                .unwrap(),
            if cfg!(target_os = "linux") {
                "TEST.ZIP"
            } else {
                "TEST (2).ZIP"
            }
        );
        library.settings = Settings {
            start_at_login: true,
            transparency: 37,
            light: true,
            russian: true,
            connections: 10,
        };
        library.save(&dir)?;
        library.save(&dir)?;
        let loaded = Library::load(&dir)?;
        assert!(loaded.entries[0].state == State::Interrupted);
        assert_eq!(loaded.folders.len(), 1);
        assert_eq!(loaded.last_folder, Some(0));
        assert!(loaded.settings == library.settings);
        assert!(!fs::read_to_string(dir.join("library.json"))?.contains("secret"));
        for scale in Scale::OPTIONS {
            library.scale = scale;
            library.save(&dir)?;
            assert!(Library::load(&dir)?.scale == scale);
        }
        assert_eq!(Scale::Auto.zoom(1.5), 1.0);
        assert_eq!(Scale::Fifty.zoom(1.5) * 1.5, 0.5);
        assert_eq!(Scale::SeventyFive.zoom(1.5) * 1.5, 0.75);
        assert_eq!(Scale::Hundred.zoom(1.5) * 1.5, 1.0);
        assert_eq!(fs::read(dir.join("test.zip"))?, b"keep");
        fs::remove_file(dir.join("test.zip"))?;
        fs::remove_file(dir.join("library.json"))?;
        fs::remove_dir(dir)?;
        Ok(())
    }
}
