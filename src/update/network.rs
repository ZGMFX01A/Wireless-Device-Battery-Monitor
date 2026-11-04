//! Update transport only. Mirrors are untrusted byte carriers; the caller verifies
//! the signed metadata and this module verifies the complete executable digest.
use reqwest::{Client, Response, StatusCode, header};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    future::Future,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;
pub const DEFAULT_MIRROR: &str = "https://gh-proxy.org/";
const METADATA_LIMIT: usize = 1024 * 1024;
const METADATA_TIMEOUT: Duration = Duration::from_secs(6);
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(15 * 60);

pub fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

pub fn run<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    let runtime = runtime()?;
    let result = runtime.block_on(future);
    // The OS resolver can still be finishing a blocking getaddrinfo call after
    // its request future is dropped. Do not keep the updater busy waiting for it.
    // This does not claim to terminate that OS call; it releases itself on return.
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}

/// Dropping the request future cancels DNS/connect/header/body waits, not just the
/// next successful read. No network operation runs on the tray or GUI thread.
async fn cancellable<T>(future: impl Future<Output = Result<T>>, cancel: &AtomicBool) -> Result<T> {
    tokio::pin!(future);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        tokio::select! {
            value = &mut future => return value,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}

pub struct Transport {
    client: Client,
    prefixes: Vec<String>,
}

#[derive(Clone, Copy)]
pub struct Expected<'a> {
    pub size: u64,
    pub sha256: &'a str,
}

fn mirror_prefix(value: &str) -> Result<String> {
    let mut u = reqwest::Url::parse(value.trim()).map_err(|_| "Invalid update mirror URL")?;
    if u.scheme() != "https"
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err(
            "Update mirrors must be HTTPS prefixes without credentials or query strings".into(),
        );
    }
    if !u.path().ends_with('/') {
        u.set_path(&format!("{}/", u.path()));
    }
    Ok(u.to_string())
}

impl Transport {
    pub fn new(mirrors: &[String]) -> Result<Self> {
        let mut prefixes = vec![String::new()];
        for value in mirrors.iter().take(3) {
            let prefix = mirror_prefix(value)?;
            if !prefixes.contains(&prefix) {
                prefixes.push(prefix);
            }
        }
        let client = Client::builder()
            .https_only(true)
            .user_agent("WirelessDeviceBatteryMonitor")
            .connect_timeout(Duration::from_secs(4))
            .read_timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::limited(6))
            .referer(false)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client, prefixes })
    }

    fn urls(&self, original: &str, preferred: usize) -> Vec<(usize, String)> {
        let mut out: Vec<_> = self
            .prefixes
            .iter()
            .enumerate()
            .map(|(i, prefix)| (i, format!("{prefix}{original}")))
            .collect();
        out.sort_by_key(|(i, _)| usize::from(*i != preferred));
        out
    }

    /// An HTTP 200 is not sufficient. A forged/obsolete mirror response must not
    /// prevent a later route from supplying metadata accepted by the caller.
    pub async fn metadata<T>(
        &self,
        url: &str,
        cancel: &AtomicBool,
        accept: impl Fn(&[u8]) -> Result<T>,
    ) -> Result<(T, usize)> {
        let mut errors = Vec::new();
        for (route, url) in self.urls(url, 0) {
            let result = cancellable(
                async {
                    let mut response = self
                        .client
                        .get(&url)
                        .header(header::ACCEPT, "application/vnd.github+json")
                        .header(header::CACHE_CONTROL, "no-cache")
                        .timeout(METADATA_TIMEOUT)
                        .send()
                        .await
                        .and_then(Response::error_for_status)
                        .map_err(|e| e.to_string())?;
                    let mut data = Vec::new();
                    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                        if data.len().saturating_add(chunk.len()) > METADATA_LIMIT {
                            return Err("oversized release response".into());
                        }
                        data.extend_from_slice(&chunk);
                    }
                    accept(&data)
                },
                cancel,
            )
            .await;
            match result {
                Ok(v) => return Ok((v, route)),
                Err(e) => {
                    if cancel.load(Ordering::Acquire) {
                        return Err("cancelled".into());
                    }
                    errors.push(format!("route {route}: {e}"));
                }
            }
        }
        Err(format!(
            "更新检查暂时无法连接，请稍后重试 / Update check failed: {}",
            errors.join("; ")
        ))
    }

    /// The partial file is keyed by the signed SHA-256, never just the version.
    /// It survives cancellation/restarts and can resume across routes. No file
    /// becomes an executable candidate before size AND digest verification.
    pub async fn download(
        &self,
        url: &str,
        preferred: usize,
        partial: &Path,
        expected: Expected<'_>,
        cancel: &AtomicBool,
        progress: impl Fn(u64),
    ) -> Result<()> {
        cancellable(
            async {
                tokio::time::timeout(
                    DOWNLOAD_LIMIT,
                    self.download_inner(url, preferred, partial, expected, cancel, &progress),
                )
                .await
                .map_err(|_| {
                    "Download time budget exhausted; partial download retained".to_owned()
                })?
            },
            cancel,
        )
        .await
    }

    async fn download_inner(
        &self,
        url: &str,
        preferred: usize,
        partial: &Path,
        expected: Expected<'_>,
        cancel: &AtomicBool,
        progress: &impl Fn(u64),
    ) -> Result<()> {
        let Expected {
            size,
            sha256: digest,
        } = expected;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(partial)
            .map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() >= size {
            if matches_file(partial, size, digest)? {
                progress(size);
                return Ok(());
            }
            file.set_len(0).map_err(|e| e.to_string())?;
        }
        let routes = self.urls(url, preferred);
        let mut unavailable = HashSet::new();
        let mut errors = Vec::new();
        for round in 0..3 {
            for (route, url) in &routes {
                if cancel.load(Ordering::Acquire) {
                    return Err("cancelled".into());
                }
                if unavailable.contains(route) {
                    continue;
                }
                let offset = file.metadata().map_err(|e| e.to_string())?.len();
                progress(offset);
                let result = async {
                    let mut request = self
                        .client
                        .get(url)
                        .header(header::ACCEPT_ENCODING, "identity");
                    if offset > 0 {
                        request = request.header(header::RANGE, format!("bytes={offset}-"));
                    }
                    let response = request.send().await.map_err(|e| e.to_string())?;
                    if matches!(response.status().as_u16(), 403 | 404 | 429) {
                        unavailable.insert(*route);
                    }
                    self.receive(response, &mut file, offset, size, progress)
                        .await
                }
                .await;
                match result {
                    Ok(()) if matches_file(partial, size, digest)? => {
                        file.sync_all().map_err(|e| e.to_string())?;
                        progress(size);
                        return Ok(());
                    }
                    Ok(()) => {
                        // A bad mirror/old same-tag cache must not poison every resume.
                        file.set_len(0).map_err(|e| e.to_string())?;
                        errors.push(format!(
                            "route {route}: hash mismatch; restarting from zero"
                        ));
                    }
                    Err(e) => {
                        // A reset can arrive after the final byte. The signed digest,
                        // not a subsequent out-of-range request, decides completeness.
                        if matches_file(partial, size, digest)? {
                            file.sync_all().map_err(|e| e.to_string())?;
                            progress(size);
                            return Ok(());
                        }
                        errors.push(format!("route {route}: {e}"));
                    }
                }
            }
            if unavailable.len() == routes.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300 * (1 << round))).await;
        }
        Err(format!(
            "下载未完成，重试将续传 / Download incomplete: {}",
            errors.join("; ")
        ))
    }

    async fn receive(
        &self,
        mut response: Response,
        file: &mut File,
        offset: u64,
        size: u64,
        progress: &impl Fn(u64),
    ) -> Result<()> {
        let status = response.status();
        let header_text = |name| response.headers().get(name).and_then(|s| s.to_str().ok());
        let (start, end) = body_range(
            status.as_u16(),
            header_text(header::CONTENT_RANGE),
            offset,
            size,
        )?;
        if header_text(header::CONTENT_TYPE).is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("text/html") || v.contains("application/json")
        }) {
            return Err("Download endpoint returned a page instead of an executable".into());
        }
        if header_text(header::CONTENT_ENCODING).is_some_and(|v| v != "identity") {
            return Err("Unexpected content encoding for executable".into());
        }
        if response
            .content_length()
            .is_some_and(|n| n != end - start + 1)
        {
            return Err("Download Content-Length does not match signed size/range".into());
        }
        if status == StatusCode::OK {
            // Range ignored: overwrite from byte zero, never append a second EXE.
            file.set_len(0).map_err(|e| e.to_string())?;
        }
        file.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        let mut n = start;
        let mut last = Instant::now();
        let began = Instant::now();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            if n.saturating_add(chunk.len() as u64) > end + 1 {
                file.set_len(start).map_err(|e| e.to_string())?;
                return Err("oversized update body".into());
            }
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            n += chunk.len() as u64;
            if last.elapsed() >= Duration::from_millis(150) || n == size {
                progress(n);
                last = Instant::now();
            }
            if self.prefixes.len() > 1
                && began.elapsed() >= Duration::from_secs(12)
                && (n - start) as f64 / began.elapsed().as_secs_f64() < 16_384.0
            {
                return Err("Slow route; switching while retaining downloaded bytes".into());
            }
        }
        if n != size {
            return Err("Incomplete range/body; resuming remaining bytes".into());
        }
        Ok(())
    }
}

fn body_range(status: u16, range: Option<&str>, offset: u64, size: u64) -> Result<(u64, u64)> {
    if size == 0 || offset >= size {
        return Err("invalid download offset".into());
    }
    if status == 200 {
        return Ok((0, size - 1));
    }
    if status != 206 {
        return Err(format!("download HTTP {status}"));
    }
    let (span, total) = range
        .and_then(|s| s.strip_prefix("bytes "))
        .and_then(|s| s.split_once('/'))
        .ok_or("Missing/invalid Content-Range")?;
    let (a, b) = span.split_once('-').ok_or("Invalid range span")?;
    let start = a.parse::<u64>().map_err(|_| "Invalid range start")?;
    let end = b.parse::<u64>().map_err(|_| "Invalid range end")?;
    if start != offset || end < start || end >= size || total.parse::<u64>().ok() != Some(size) {
        return Err("Content-Range does not match the signed executable".into());
    }
    Ok((start, end))
}

fn matches_file(path: &Path, size: u64, digest: &str) -> Result<bool> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() != size {
        return Ok(false);
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut b = [0u8; 65536];
    loop {
        let n = file.read(&mut b).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    Ok(hex::encode(h.finalize()).eq_ignore_ascii_case(digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn strict_ranges_and_ignored_range_restart() {
        assert_eq!(body_range(200, None, 3, 10).unwrap(), (0, 9));
        assert_eq!(
            body_range(206, Some("bytes 3-9/10"), 3, 10).unwrap(),
            (3, 9)
        );
        for r in [
            None,
            Some("bytes 0-9/10"),
            Some("bytes 3-11/12"),
            Some("bytes 4-3/10"),
            Some("bytes 3-9/*"),
        ] {
            assert!(body_range(206, r, 3, 10).is_err());
        }
        assert!(body_range(416, None, 3, 10).is_err());
    }
    #[test]
    fn mirrors_are_optional_and_never_receive_credentials() {
        assert!(mirror_prefix("http://mirror.example").is_err());
        assert!(mirror_prefix("https://user:secret@mirror.example").is_err());
        assert!(mirror_prefix("https://mirror.example/?token=x").is_err());
        assert_eq!(
            mirror_prefix("https://mirror.example/base").unwrap(),
            "https://mirror.example/base/"
        );
        let n = Transport::new(&[]).unwrap();
        assert_eq!(
            n.urls("https://github.com/file", 0),
            vec![(0, "https://github.com/file".into())]
        );
    }
    fn server(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut data = Vec::new();
                let mut byte = [0u8];
                while !data.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    data.push(byte[0]);
                }
                requests.push(String::from_utf8(data).unwrap());
                stream.write_all(response.as_bytes()).unwrap();
            }
            requests
        });
        (url, thread)
    }
    fn local_transport() -> Transport {
        Transport {
            client: Client::builder().no_proxy().build().unwrap(),
            prefixes: vec![String::new()],
        }
    }
    #[test]
    fn interrupted_transfer_resumes_from_actual_bytes() {
        let (url, server) = server(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabc".into(),
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\ndef".into(),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("part");
        runtime()
            .unwrap()
            .block_on(local_transport().download(
                &url,
                0,
                &part,
                Expected {
                    size: 6,
                    sha256: &hex::encode(Sha256::digest(b"abcdef")),
                },
                &AtomicBool::new(false),
                |_| {},
            ))
            .unwrap();
        assert_eq!(fs::read(part).unwrap(), b"abcdef");
        let requests = server.join().unwrap();
        assert!(requests[1].to_lowercase().contains("range: bytes=3-"));
    }
    #[test]
    fn ignored_range_does_not_append_two_files() {
        let (url, server) = server(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef".into(),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("part");
        fs::write(&part, b"abc").unwrap();
        runtime()
            .unwrap()
            .block_on(local_transport().download(
                &url,
                0,
                &part,
                Expected {
                    size: 6,
                    sha256: &hex::encode(Sha256::digest(b"abcdef")),
                },
                &AtomicBool::new(false),
                |_| {},
            ))
            .unwrap();
        assert_eq!(fs::read(part).unwrap(), b"abcdef");
        server.join().unwrap();
    }
    #[test]
    fn pending_io_is_cancellable_without_waiting_for_timeout() {
        let cancel = AtomicBool::new(false);
        let start = Instant::now();
        runtime().unwrap().block_on(async {
            let future = async {
                tokio::time::sleep(Duration::from_millis(80)).await;
                cancel.store(true, Ordering::Release);
            };
            let request = cancellable(std::future::pending::<Result<()>>(), &cancel);
            let (_, result) = tokio::join!(future, request);
            assert!(result.is_err());
        });
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn cancelled_request_does_not_wait_for_a_background_resolver() {
        let start = Instant::now();
        let result: Result<()> = run(async {
            let _resolver =
                tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_secs(1)));
            tokio::time::sleep(Duration::from_millis(30)).await;
            Err("cancelled".into())
        });
        assert_eq!(result.unwrap_err(), "cancelled");
        assert!(start.elapsed() < Duration::from_millis(750));
    }

    #[test]
    fn invalid_success_metadata_falls_through_to_the_next_route() {
        let (direct, a) = server(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nforged".into(),
        ]);
        let (mirror, b) = server(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\napproved".into(),
        ]);
        let mut t = local_transport();
        t.prefixes.push(mirror);
        let (value, route) = runtime()
            .unwrap()
            .block_on(t.metadata(&direct, &AtomicBool::new(false), |bytes| {
                if bytes == b"approved" {
                    Ok("accepted")
                } else {
                    Err("invalid signature fixture".into())
                }
            }))
            .unwrap();
        assert_eq!((value, route), ("accepted", 1));
        a.join().unwrap();
        b.join().unwrap();
    }

    #[test]
    fn corrupt_resumed_prefix_is_discarded_before_a_clean_retry() {
        let (url, server) = server(vec![
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\ndef".into(),
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef".into(),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("part");
        fs::write(&part, b"BAD").unwrap();
        runtime()
            .unwrap()
            .block_on(local_transport().download(
                &url,
                0,
                &part,
                Expected {
                    size: 6,
                    sha256: &hex::encode(Sha256::digest(b"abcdef")),
                },
                &AtomicBool::new(false),
                |_| {},
            ))
            .unwrap();
        assert_eq!(fs::read(part).unwrap(), b"abcdef");
        let requests = server.join().unwrap();
        assert!(requests[0].to_lowercase().contains("range: bytes=3-"));
        assert!(!requests[1].to_lowercase().contains("range:"));
    }

    #[test]
    fn rate_limit_does_not_hot_loop_the_same_route() {
        let (url, server) = server(vec![
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .into(),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let result = runtime().unwrap().block_on(local_transport().download(
            &url,
            0,
            &dir.path().join("part"),
            Expected {
                size: 6,
                sha256: &hex::encode(Sha256::digest(b"abcdef")),
            },
            &AtomicBool::new(false),
            |_| {},
        ));
        assert!(result.is_err());
        assert_eq!(server.join().unwrap().len(), 1);
    }

    #[test]
    fn cancelling_a_stalled_socket_retains_partial_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut data = Vec::new();
            let mut b = [0u8];
            while !data.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut b).unwrap();
                data.push(b[0]);
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nabc",
                )
                .unwrap();
            std::thread::sleep(Duration::from_millis(150));
            signal.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(700));
        });
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("part");
        let start = Instant::now();
        let result = runtime().unwrap().block_on(local_transport().download(
            &url,
            0,
            &part,
            Expected {
                size: 100,
                sha256: &"00".repeat(32),
            },
            &cancel,
            |_| {},
        ));
        assert_eq!(result.unwrap_err(), "cancelled");
        assert!(start.elapsed() < Duration::from_millis(650));
        assert_eq!(fs::read(part).unwrap(), b"abc");
        server.join().unwrap();
    }
}
