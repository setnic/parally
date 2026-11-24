use anyhow::Result;
use std::{
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::HWND,
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree, CoUninitialize, IBindCtx,
        },
        UI::Shell::{
            Common::COMDLG_FILTERSPEC, FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
            FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog,
            IFileDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
        },
    },
    core::{HRESULT, IUnknown, PCWSTR, w},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Save,
    Partial,
    Folder,
}

pub struct Picker {
    pub kind: Kind,
    path: PathBuf,
    suggested: String,
}

struct Apartment;
impl Apartment {
    fn enter() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

impl Picker {
    pub fn new(kind: Kind, destination: &str, suggested: &str) -> Self {
        Self {
            kind,
            path: PathBuf::from(destination.trim()),
            suggested: suggested.into(),
        }
    }

    pub fn show(&self, owner: windows_sys::Win32::Foundation::HWND) -> Result<Option<PathBuf>> {
        let _com = Apartment::enter()?;
        let dialog = self.configure()?;
        let owner = (!owner.is_null()).then_some(HWND(owner));
        if let Err(error) = unsafe { dialog.Show(owner) } {
            if error.code() == HRESULT(0x800704c7_u32 as i32) {
                return Ok(None);
            }
            return Err(error.into());
        }
        let path = unsafe { dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)? };
        let result = unsafe { PathBuf::from(OsString::from_wide(path.as_wide())) };
        unsafe { CoTaskMemFree(Some(path.0.cast())) };
        Ok(Some(result))
    }

    fn configure(&self) -> windows::core::Result<IFileDialog> {
        let clsid = if self.kind == Kind::Save {
            &FileSaveDialog
        } else {
            &FileOpenDialog
        };
        let dialog: IFileDialog =
            unsafe { CoCreateInstance(clsid, None::<&IUnknown>, CLSCTX_INPROC_SERVER)? };
        let mut options =
            FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR | FOS_DONTADDTORECENT | FOS_PATHMUSTEXIST;
        let title = match self.kind {
            Kind::Save => "Save download as",
            Kind::Partial => {
                options |= FOS_FILEMUSTEXIST;
                "Open partial download"
            }
            Kind::Folder => {
                options |= FOS_PICKFOLDERS;
                "Add folder preset"
            }
        };
        unsafe {
            dialog.SetOptions(options)?;
            dialog.SetTitle(PCWSTR(wide(OsStr::new(crate::i18n::text(title))).as_ptr()))?;
            if self.kind == Kind::Partial {
                dialog.SetFileTypes(&[COMDLG_FILTERSPEC {
                    pszName: w!("Resume files"),
                    pszSpec: w!("resume.json"),
                }])?;
            }
            if self.kind == Kind::Save {
                let name = self
                    .path
                    .file_name()
                    .filter(|_| !self.path.is_dir())
                    .unwrap_or(self.suggested.as_ref());
                if !name.is_empty() {
                    dialog.SetFileName(PCWSTR(wide(name).as_ptr()))?;
                }
            }
            let start = if self.path.is_dir() {
                Some(self.path.as_path())
            } else {
                self.path.parent()
            };
            if let Some(folder) = start.filter(|path| path.is_absolute() && path.is_dir())
                && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                    PCWSTR(wide(Path::as_os_str(folder)).as_ptr()),
                    None::<&IBindCtx>,
                )
            {
                let _ = dialog.SetFolder(&item);
            }
        }
        Ok(dialog)
    }
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configures_all_three_windows_dialogs_without_showing_them() -> Result<()> {
        let _com = Apartment::enter()?;
        for kind in [Kind::Save, Kind::Partial, Kind::Folder] {
            let dialog = Picker::new(kind, "", "sample.zip").configure()?;
            let options = unsafe { dialog.GetOptions()? };
            assert!(options.contains(FOS_FORCEFILESYSTEM));
            assert_eq!(options.contains(FOS_PICKFOLDERS), kind == Kind::Folder);
            if kind == Kind::Partial {
                assert!(options.contains(FOS_FILEMUSTEXIST));
            }
            if kind == Kind::Save {
                assert!(!options.contains(windows::Win32::UI::Shell::FOS_OVERWRITEPROMPT));
                let name = unsafe { dialog.GetFileName()? };
                assert_eq!(unsafe { name.to_string()? }, "sample.zip");
                unsafe { CoTaskMemFree(Some(name.0.cast())) };
            }
        }
        Ok(())
    }
}
