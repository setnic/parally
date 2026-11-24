use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Save,
    Folder,
}

pub struct Picker {
    pub kind: Kind,
    path: PathBuf,
    suggested: String,
}

impl Picker {
    pub fn new(kind: Kind, destination: &str, suggested: &str) -> Self {
        Self {
            kind,
            path: PathBuf::from(destination.trim()),
            suggested: suggested.into(),
        }
    }

    pub fn show(&self) -> Result<Option<PathBuf>> {
        let title = crate::i18n::text(match self.kind {
            Kind::Save => "Save download as",
            Kind::Folder => "Add folder preset",
        });
        let folder = if self.path.is_dir() {
            Some(self.path.as_path())
        } else {
            self.path.parent()
        };
        let folder = folder
            .filter(|path| path.is_absolute() && path.is_dir())
            .unwrap_or(Path::new("/"));
        let name = self
            .path
            .file_name()
            .filter(|_| !self.path.is_dir())
            .unwrap_or(self.suggested.as_ref());
        #[cfg(target_os = "linux")]
        let output = {
            let mut dialog = Command::new("zenity");
            dialog.args(["--file-selection", "--title", title]);
            match self.kind {
                Kind::Save => {
                    dialog
                        .arg("--save")
                        .arg(format!("--filename={}", folder.join(name).display()));
                }
                Kind::Folder => {
                    dialog
                        .arg("--directory")
                        .arg(format!("--filename={}/", folder.display()));
                }
            }
            dialog
                .output()
                .context("Could not open the file chooser (Zenity is required)")?
        };
        #[cfg(target_os = "macos")]
        let output = {
            let choose = match self.kind {
                Kind::Save => {
                    "choose file name with prompt item 1 of argv default location startDir default name item 3 of argv"
                }
                Kind::Folder => {
                    "choose folder with prompt item 1 of argv default location startDir"
                }
            };
            let script = format!(
                "on run argv\nset startDir to POSIX file (item 2 of argv) as alias\nset resultPath to {choose}\nreturn POSIX path of resultPath\nend run"
            );
            Command::new("osascript")
                .args(["-e", &script, title])
                .arg(folder)
                .arg(name)
                .output()
                .context("Could not open the macOS file chooser")?
        };
        if output.status.success() {
            let path = String::from_utf8(output.stdout)?
                .trim_end_matches(['\n', '\r'])
                .to_owned();
            Ok(Some(PathBuf::from(path)))
        } else if (cfg!(target_os = "linux") && output.status.code() == Some(1))
            || String::from_utf8_lossy(&output.stderr).contains("(-128)")
        {
            Ok(None)
        } else {
            anyhow::bail!(
                "Could not open the file chooser: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        }
    }
}
