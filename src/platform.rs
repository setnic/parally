use anyhow::{Context, Result, ensure};
use std::{path::PathBuf, process::Command};

pub fn app_data() -> Result<PathBuf> {
    #[cfg(windows)]
    return std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Parally"))
        .context("LocalAppData is unavailable");

    #[cfg(not(windows))]
    {
        let home =
            PathBuf::from(std::env::var_os("HOME").context("The home folder is unavailable")?);
        #[cfg(target_os = "macos")]
        return Ok(home.join("Library/Application Support/Parally"));
        #[cfg(target_os = "linux")]
        return Ok(std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"))
            .join("Parally"));
    }
}

#[cfg(windows)]
pub fn set_startup(enabled: bool) -> Result<()> {
    use std::{os::windows::process::CommandExt, process::Stdio};
    const RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let mut command = Command::new("reg.exe");
    command
        .creation_flags(0x0800_0000)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let executable = std::env::current_exe()?;
    let path = format!("\"{}\"", executable.display());
    let args = if enabled {
        vec![
            "add", RUN, "/v", "Parally", "/t", "REG_SZ", "/d", &path, "/f",
        ]
    } else {
        vec!["delete", RUN, "/v", "Parally", "/f"]
    };
    let success = command.args(args).status()?.success();
    if !enabled && !success {
        let exists = Command::new("reg.exe")
            .creation_flags(0x0800_0000)
            .args(["query", RUN, "/v", "Parally"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success();
        if !exists {
            return Ok(());
        }
    }
    ensure!(success, "Could not change startup settings.");
    Ok(())
}

#[cfg(not(windows))]
pub fn set_startup(enabled: bool) -> Result<()> {
    use std::fs;
    let home = PathBuf::from(std::env::var_os("HOME").context("The home folder is unavailable")?);
    let executable = std::env::current_exe()?;
    let executable = executable
        .to_str()
        .context("The app path is not valid Unicode")?;
    #[cfg(target_os = "macos")]
    let (path, content) = (
        home.join("Library/LaunchAgents/com.parally.app.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Label</key><string>com.parally.app</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/></dict></plist>"#,
            executable
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        ),
    );
    #[cfg(target_os = "linux")]
    let (path, content) = {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        ensure!(
            !executable.contains(['\n', '\r']),
            "The app path contains a line break."
        );
        let escaped = executable
            .replace('\\', "\\\\\\\\")
            .replace('"', "\\\\\"")
            .replace('`', "\\\\`")
            .replace('$', "\\\\$")
            .replace('%', "%%");
        (
            config.join("autostart/com.parally.app.desktop"),
            format!("[Desktop Entry]\nType=Application\nName=Parally\nExec=\"{escaped}\"\n"),
        )
    };
    if enabled {
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, content)?;
    } else if let Err(error) = fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn open_path(path: &std::path::Path) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(path)
        .spawn()
        .context("Could not open the file or folder")?;
    Ok(())
}

/// Fixed by the `key` in browser/extension/manifest.json, so it does not depend on the folder.
pub const EXTENSION_ID: &str = "ojajnbkdcdgameeaaddbcbcabjgfdoeo";
const HOST: &str = "com.parally.download";

/// The bundled extension folder that the user loads with Load unpacked.
pub fn extension_folder() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    let folder = PathBuf::from("/usr/share/parally/browser/extension");
    // Installed: {app}\release-x\Parally.exe or Parally.app/Contents/MacOS/Parally;
    // a source build runs from target\debug, a few folders below browser\.
    #[cfg(not(target_os = "linux"))]
    let folder = std::env::current_exe()?
        .ancestors()
        .skip(1)
        .take(4)
        .map(|folder| {
            folder.join(if cfg!(target_os = "macos") {
                "Resources/browser/extension"
            } else {
                "browser/extension"
            })
        })
        .find(|folder| folder.join("manifest.json").is_file())
        .context("The browser extension is missing from this installation.")?;
    #[cfg(target_os = "linux")]
    ensure!(
        folder.join("manifest.json").is_file(),
        "The browser extension is missing from this installation."
    );
    Ok(folder)
}

/// Points Chromium browsers at this Parally executable; runs on every start, so a moved app keeps working.
pub fn register_browsers(install: bool) -> Result<()> {
    let manifest = serde_json::json!({
        "name": HOST,
        "description": "Parally download handoff",
        "path": std::env::current_exe()?,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{EXTENSION_ID}/")],
    });
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::ERROR_FILE_NOT_FOUND,
            System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyW, RegSetKeyValueW},
        };
        let file = app_data()?.join("native-host.json");
        if install {
            std::fs::create_dir_all(app_data()?)?;
            crate::library::write_json(&file, &manifest)?;
        }
        let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let value = wide(&file.to_string_lossy());
        // Opera reads the Chrome key.
        for vendor in [
            r"Google\Chrome",
            "Chromium",
            r"BraveSoftware\Brave-Browser",
            r"Microsoft\Edge",
            "Vivaldi",
        ] {
            let key = wide(&format!(r"Software\{vendor}\NativeMessagingHosts\{HOST}"));
            let status = unsafe {
                if install {
                    RegSetKeyValueW(
                        HKEY_CURRENT_USER,
                        key.as_ptr(),
                        std::ptr::null(),
                        REG_SZ,
                        value.as_ptr().cast(),
                        (value.len() * 2) as u32,
                    )
                } else {
                    RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr())
                }
            };
            ensure!(
                status == 0 || (!install && status == ERROR_FILE_NOT_FOUND),
                "Could not connect Parally to {vendor}."
            );
        }
        if !install
            && let Err(error) = std::fs::remove_file(file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        use std::fs;
        let home =
            PathBuf::from(std::env::var_os("HOME").context("The home folder is unavailable")?);
        #[cfg(target_os = "macos")]
        let (config, vendors) = (
            home.join("Library/Application Support"),
            [
                "Google/Chrome",
                "Chromium",
                "BraveSoftware/Brave-Browser",
                "Microsoft Edge",
                "Vivaldi",
            ],
        );
        #[cfg(target_os = "linux")]
        let (config, vendors) = (
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| home.join(".config")),
            [
                "google-chrome",
                "chromium",
                "BraveSoftware/Brave-Browser",
                "microsoft-edge",
                "vivaldi",
            ],
        );
        for vendor in vendors {
            let path = config
                .join(vendor)
                .join(format!("NativeMessagingHosts/{HOST}.json"));
            if install {
                fs::create_dir_all(path.parent().unwrap())?;
                crate::library::write_json(&path, &manifest)?;
            } else if let Err(error) = fs::remove_file(path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err(error.into());
            }
        }
        Ok(())
    }
}
