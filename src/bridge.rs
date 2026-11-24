use crate::library;
use anyhow::{Context, Result, ensure};
use parallel_downloader::{BrowserHeaders, validate_url};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const LIMIT: usize = 32 * 1024;
const LIFETIME: u64 = 300;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub url: String,
    pub filename: String,
    /// The page the link came from, sent as Referer.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub referrer: String,
    /// The browser's cookies for `url`, used for this download only and never saved.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cookie: String,
    /// Sent from the link menu: no browser download waits for the answer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub manual: bool,
}

impl Offer {
    fn validate(&self) -> Result<()> {
        validate_url(&self.url)?;
        ensure!(
            self.referrer.is_empty() || validate_url(&self.referrer).is_ok(),
            "Invalid browser page address."
        );
        ensure!(
            !self.cookie.chars().any(char::is_control),
            "Invalid browser cookie."
        );
        ensure!(
            self.filename.len() <= 1024 && !self.filename.chars().any(char::is_control),
            "Invalid browser filename."
        );
        Ok(())
    }

    pub fn headers(&self) -> BrowserHeaders {
        BrowserHeaders {
            cookie: self.cookie.clone(),
            referrer: self.referrer.clone(),
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct Request {
    pub offer: Offer,
    expires: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

impl Request {
    pub fn live(&self) -> bool {
        now() < self.expires && self.path.is_file() && !self.reply_path().is_file()
    }
    pub fn reply(&self, accepted: bool) -> Result<bool> {
        if self.live() {
            library::write_json(&self.reply_path(), &Reply { accepted })?;
            return Ok(true);
        }
        Ok(false)
    }
    fn reply_path(&self) -> PathBuf {
        self.path.with_extension("reply")
    }
}

#[derive(Deserialize, Serialize)]
struct Reply {
    accepted: bool,
}

pub fn offers(root: &Path) -> Vec<Request> {
    let Ok(files) = fs::read_dir(root.join("inbox")) else {
        return Vec::new();
    };
    let mut offers = Vec::new();
    for file in files.flatten() {
        let path = file.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "offer")
            || !file.file_type().is_ok_and(|kind| kind.is_file())
            || file
                .metadata()
                .map_or(true, |meta| meta.len() > LIMIT as u64 + 1024)
        {
            continue;
        }
        if let Ok(mut request) = fs::File::open(&path)
            .and_then(|file| serde_json::from_reader::<_, Request>(file).map_err(io::Error::other))
        {
            request.path = path;
            if request.expires < now().saturating_sub(LIFETIME) {
                let _ = fs::remove_file(&request.path);
                let _ = fs::remove_file(request.reply_path());
            } else if request.live() && request.offer.validate().is_ok() {
                offers.push(request);
            }
        }
    }
    offers.sort_by(|left, right| left.path.cmp(&right.path));
    offers
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_offer(input: &mut impl Read) -> Result<Offer> {
    let mut length = [0; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!((1..=LIMIT).contains(&length), "Invalid message size.");
    let mut contents = vec![0; length];
    input.read_exact(&mut contents)?;
    let offer: Offer = serde_json::from_slice(&contents)?;
    offer.validate()?;
    Ok(offer)
}

fn send_reply(output: &mut impl Write, accepted: bool) -> Result<()> {
    let data = serde_json::to_vec(&Reply { accepted })?;
    output.write_all(&(data.len() as u32).to_le_bytes())?;
    output.write_all(&data)?;
    output.flush()?;
    Ok(())
}

fn handoff(offer: Offer, disconnected: mpsc::Receiver<()>) -> Result<bool> {
    let root = library::root()?;
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let path = root
        .join("inbox")
        .join(format!("{unique}-{}.offer", std::process::id()));
    let request = Request {
        offer,
        expires: now() + LIFETIME,
        path,
    };
    library::write_json(&request.path, &request)?;
    let outcome = (|| {
        if let Some(lock) = library::try_instance_lock(&root)? {
            drop(lock);
            Command::new(std::env::current_exe()?)
                .arg("--browser-request")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .context("Could not start Parally")?;
        }
        while now() < request.expires {
            if let Ok(file) = fs::File::open(request.reply_path()) {
                return Ok(serde_json::from_reader::<_, Reply>(file)?.accepted);
            }
            if disconnected
                .recv_timeout(Duration::from_millis(100))
                .is_ok()
            {
                return Ok(false);
            }
        }
        Ok(false)
    })();
    let _ = fs::remove_file(&request.path);
    let _ = fs::remove_file(request.reply_path());
    outcome
}

pub fn native_host() {
    let mut input = io::stdin();
    let accepted = read_offer(&mut input)
        .and_then(|offer| {
            let (signal, disconnected) = mpsc::channel();
            thread::spawn(move || {
                let _ = input.read_exact(&mut [0]);
                let _ = signal.send(());
            });
            handoff(offer, disconnected)
        })
        .unwrap_or(false);
    let _ = send_reply(&mut io::stdout(), accepted);
}

pub fn native_origin(argument: &str) -> bool {
    let origin = argument
        .strip_prefix("chrome-extension://")
        .and_then(|value| value.strip_suffix('/'));
    origin.is_some_and(|id| {
        id.len() == 32 && id.bytes().all(|letter| (b'a'..=b'p').contains(&letter))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_frames_reject_credentials_and_header_injection() -> Result<()> {
        let mut reply = Vec::new();
        send_reply(&mut reply, true)?;
        assert_eq!(
            u32::from_le_bytes(reply[..4].try_into()?) as usize,
            reply.len() - 4
        );
        assert!(serde_json::from_slice::<Reply>(&reply[4..])?.accepted);
        for (url, referrer, cookie, valid) in [
            ("http://localhost/file?sig=1", "https://localhost/page", "a=1; b=2", true),
            ("blob:http://localhost/123", "", "", false),
            ("http://user:pass@localhost/file", "", "", false),
            ("http://localhost/file", "javascript:alert(1)", "", false),
            ("http://localhost/file", "", "a=1\r\nX-Injected: 1", false),
        ] {
            let body = serde_json::to_vec(&Offer {
                url: url.into(),
                filename: "file".into(),
                referrer: referrer.into(),
                cookie: cookie.into(),
                manual: false,
            })?;
            let mut frame = (body.len() as u32).to_le_bytes().to_vec();
            frame.extend(body);
            assert_eq!(read_offer(&mut &frame[..]).is_ok(), valid, "{url} {cookie}");
        }
        Ok(())
    }
}
