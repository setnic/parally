use crate::{BrowserHeaders, read_network, storage::Remote, validate_url};
use anyhow::{Result, bail, ensure};
use std::time::Duration;
use ureq::{Agent, Error, Response};

#[derive(Debug)]
pub struct RangeIssue;

impl std::fmt::Display for RangeIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The server returned an inconsistent range response.")
    }
}

impl std::error::Error for RangeIssue {}

#[derive(Debug)]
pub struct Retryable(pub String);

impl std::fmt::Display for Retryable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Retryable {}

pub enum Probe {
    Full(Option<Box<Response>>),
    Range(Remote),
}

pub fn client(browser: &BrowserHeaders) -> Agent {
    let builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(8))
        .timeout_write(Duration::from_secs(8))
        .user_agent(concat!("Parally/", env!("CARGO_PKG_VERSION")));
    let headers: Vec<_> = [("Cookie", &browser.cookie), ("Referer", &browser.referrer)]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| (name, value.clone()))
        .collect();
    if headers.is_empty() {
        return builder.build();
    }
    // ureq drops Cookie on every redirect, so the cookie only reaches the URL the browser named.
    builder
        .middleware(move |request: ureq::Request, next: ureq::MiddlewareNext| {
            let request = headers
                .iter()
                .fold(request, |request, (name, value)| request.set(name, value));
            next.handle(request)
        })
        .build()
}

fn request(
    agent: &Agent,
    url: &str,
    range: Option<(u64, u64)>,
    etag: Option<&str>,
) -> Result<Response> {
    let mut request = agent.get(url).set("Accept-Encoding", "identity");
    if let Some((start, end)) = range {
        request = request.set("Range", &format!("bytes={start}-{}", end - 1));
    }
    if let Some(etag) = etag {
        request = request.set("If-Range", etag);
    }
    match request.call() {
        Ok(response) => {
            validate_url(response.get_url())?;
            Ok(response)
        }
        Err(Error::Status(code, _))
            if range.is_some() && matches!(code, 400 | 405 | 412 | 416 | 501) =>
        {
            Err(RangeIssue.into())
        }
        Err(Error::Status(code, _)) if matches!(code, 408 | 429 | 500 | 502 | 503 | 504) => {
            Err(Retryable(format!("The server returned HTTP {code}.")).into())
        }
        Err(Error::Status(code, _)) => bail!("The server returned HTTP {code}."),
        Err(Error::Transport(error)) => {
            let kind = error.kind();
            if matches!(
                kind,
                ureq::ErrorKind::Io | ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Dns
            ) {
                return Err(Retryable(format!("Could not reach the server ({kind:?}).")).into());
            }
            bail!("The request failed ({kind:?}).");
        }
    }
}

pub fn get(agent: &Agent, url: &str) -> Result<Response> {
    request(agent, url, None, None)
}

pub fn probe(agent: &Agent, url: &str, allow_html: bool) -> Result<Probe> {
    let response = match request(agent, url, Some((0, 1)), None) {
        Ok(response) => response,
        Err(error) if error.is::<RangeIssue>() => return Ok(Probe::Full(None)),
        Err(error) => return Err(error),
    };
    check_content_type(&response, allow_html)?;
    if response.status() == 200 {
        return Ok(Probe::Full(Some(Box::new(response))));
    }
    let Ok(size) = range_length(&response, 0, 1) else {
        return Ok(Probe::Full(None));
    };
    let tags = response.all("ETag");
    let [etag] = tags.as_slice() else {
        return Ok(Probe::Full(None));
    };
    if !strong_etag(etag) {
        return Ok(Probe::Full(None));
    }
    let remote = Remote {
        url: response.get_url().to_owned(),
        etag: (*etag).to_owned(),
        size,
    };
    let mut body = response.into_reader();
    let mut byte = [0; 1];
    if read_network(&mut body, &mut byte)? != 1 {
        return Err(Retryable("The server ended the range probe early.".into()).into());
    }
    if read_network(&mut body, &mut byte)? != 0 {
        return Ok(Probe::Full(None));
    }
    Ok(Probe::Range(remote))
}

pub fn check_content_type(response: &Response, allow_html: bool) -> Result<()> {
    ensure!(
        allow_html
            || !response.all("Content-Type").iter().any(|value| matches!(
                value
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "text/html" | "application/xhtml+xml"
            )),
        "The server returned a web page. Open the link in your browser and copy the direct file download link."
    );
    Ok(())
}

pub fn segment(agent: &Agent, remote: &Remote, start: u64, end: u64) -> Result<Response> {
    let response = request(agent, &remote.url, Some((start, end)), Some(&remote.etag))?;
    ensure!(
        response.get_url() == remote.url
            && response.all("ETag") == [remote.etag.as_str()]
            && range_length(&response, start, end)? == remote.size,
        RangeIssue
    );
    Ok(response)
}

pub fn full_length(response: &Response) -> Result<Option<u64>> {
    ensure!(
        response.status() == 200 && response.all("Content-Range").is_empty(),
        "Expected HTTP 200 without a Content-Range for a full download."
    );
    body_length(response)
        .ok_or_else(|| anyhow::anyhow!("The server returned invalid or unsupported body headers."))
}

fn range_length(response: &Response, start: u64, end: u64) -> Result<u64> {
    ensure!(response.status() == 206, RangeIssue);
    let ranges = response.all("Content-Range");
    let [range] = ranges.as_slice() else {
        return Err(RangeIssue.into());
    };
    let (unit, range) = range.split_once(' ').ok_or(RangeIssue)?;
    let (coordinates, size) = range.split_once('/').ok_or(RangeIssue)?;
    let (first, last) = coordinates.split_once('-').ok_or(RangeIssue)?;
    let size = decimal(size).ok_or(RangeIssue)?;
    ensure!(
        unit.eq_ignore_ascii_case("bytes")
            && decimal(first) == Some(start)
            && decimal(last) == Some(end - 1)
            && end <= size
            && body_length(response)
                .ok_or(RangeIssue)?
                .is_none_or(|length| length == end - start)
            && !response.all("Content-Type").iter().any(|value| value
                .to_ascii_lowercase()
                .starts_with("multipart/byteranges")),
        RangeIssue
    );
    Ok(size)
}

fn decimal(value: &str) -> Option<u64> {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| value.parse().ok())
        .flatten()
}

fn body_length(response: &Response) -> Option<Option<u64>> {
    let encoding = response.all("Content-Encoding");
    if !encoding.is_empty()
        && !matches!(encoding.as_slice(), [value] if value.eq_ignore_ascii_case("identity"))
    {
        return None;
    }
    let lengths = response.all("Content-Length");
    let transfer = response.all("Transfer-Encoding");
    match (lengths.as_slice(), transfer.as_slice()) {
        ([], []) => Some(None),
        ([], [value]) if value.eq_ignore_ascii_case("chunked") => Some(None),
        ([value], []) => decimal(value).map(Some),
        _ => None,
    }
}

pub fn strong_etag(tag: &str) -> bool {
    tag.strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .is_some_and(|value| {
            value
                .bytes()
                .all(|byte| byte == 0x21 || (0x23..=0x7e).contains(&byte))
        })
}
