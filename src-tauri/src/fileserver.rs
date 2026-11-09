//! Minimal loopback file server for media playback.
//!
//! Tauri's `asset://` protocol stalls the WebView2 media pipeline: the same
//! 1080p30 file played at 13 fps with 69 dropped frames over `asset://`, and
//! at 28 fps with zero drops over plain HTTP on the same machine. The player
//! therefore streams from a tiny HTTP/1.1 server bound to 127.0.0.1.
//!
//! Only files explicitly registered by the app are served, each behind an
//! unguessable token, so the port cannot be used to read arbitrary paths.
//! Range requests are supported because video players seek with them.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static PORT: OnceLock<u16> = OnceLock::new();
static REGISTRY: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn registry() -> &'static Mutex<HashMap<String, PathBuf>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Starts the server on a random loopback port (idempotent). Returns the port.
pub fn start() -> u16 {
    *PORT.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").expect("cannot bind media server");
        let port = listener.local_addr().expect("no local addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(stream) = stream {
                    std::thread::spawn(move || {
                        let _ = handle(stream);
                    });
                }
            }
        });
        port
    })
}

/// Registers a file and returns a URL the webview can play.
pub fn register(path: &std::path::Path) -> String {
    let port = start();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let token = format!(
        "{:016x}{:016x}",
        nanos as u64 ^ (n.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        (nanos >> 64) as u64 ^ n
    );
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(token.clone(), path.to_path_buf());
    format!("http://127.0.0.1:{port}/f/{token}")
}

fn content_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "mp4" | "m4v" | "mov" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" | "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_nodelay(true)?;
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut range_header: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                range_header = Some(value.trim().to_string());
            }
        }
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");

    if method != "GET" && method != "HEAD" {
        return respond(&mut stream, 405, "Method Not Allowed", &[], &[]);
    }

    let token = match target.strip_prefix("/f/") {
        Some(t) => t.split('?').next().unwrap_or(t),
        None => return respond(&mut stream, 404, "Not Found", &[], &[]),
    };

    let path = {
        let map = registry().lock().unwrap_or_else(|e| e.into_inner());
        map.get(token).cloned()
    };
    let Some(path) = path else {
        return respond(&mut stream, 404, "Not Found", &[], &[]);
    };

    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return respond(&mut stream, 404, "Not Found", &[], &[]),
    };
    let len = file.metadata()?.len();

    // Single-range support: `bytes=start-end`, `bytes=start-`, `bytes=-suffix`.
    let (status, start, end) = match range_header.as_deref().and_then(parse_range) {
        Some((start, end)) => {
            let start = start.min(len.saturating_sub(1));
            let end = end.unwrap_or(len.saturating_sub(1)).min(len.saturating_sub(1));
            if start > end {
                return respond(&mut stream, 416, "Range Not Satisfiable", &[], &[]);
            }
            (206, start, end)
        }
        None => (200, 0, len.saturating_sub(1)),
    };

    let count = if len == 0 { 0 } else { end - start + 1 };
    let mut headers = vec![
        format!("Content-Type: {}", content_type(&path)),
        format!("Content-Length: {count}"),
        "Accept-Ranges: bytes".to_string(),
        "Cache-Control: no-store".to_string(),
        "Access-Control-Allow-Origin: *".to_string(),
        "Connection: close".to_string(),
    ];
    if status == 206 {
        headers.push(format!("Content-Range: bytes {start}-{end}/{len}"));
    }

    let head = format!(
        "HTTP/1.1 {status} {}\r\n{}\r\n\r\n",
        if status == 206 { "Partial Content" } else { "OK" },
        headers.join("\r\n")
    );
    stream.write_all(head.as_bytes())?;

    if method == "HEAD" || count == 0 {
        return stream.flush();
    }

    file.seek(SeekFrom::Start(start))?;
    let mut remaining = count;
    let mut buf = vec![0u8; 256 * 1024];
    while remaining > 0 {
        let want = remaining.min(buf.len() as u64) as usize;
        let read = file.read(&mut buf[..want])?;
        if read == 0 {
            break;
        }
        stream.write_all(&buf[..read])?;
        remaining -= read as u64;
    }
    stream.flush()
}

fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let spec = value.strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    if a.is_empty() {
        // Suffix range: last N bytes.
        let n: u64 = b.trim().parse().ok()?;
        return Some((0, Some(n.saturating_sub(1))));
    }
    let start: u64 = a.trim().parse().ok()?;
    let end = if b.trim().is_empty() {
        None
    } else {
        Some(b.trim().parse().ok()?)
    };
    Some((start, end))
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    headers: &[String],
    body: &[u8],
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for h in headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::parse_range;

    #[test]
    fn parses_closed_ranges() {
        assert_eq!(parse_range("bytes=0-1023"), Some((0, Some(1023))));
    }

    #[test]
    fn parses_open_ended_ranges() {
        assert_eq!(parse_range("bytes=500-"), Some((500, None)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_range("items=0-1"), None);
        assert_eq!(parse_range("bytes=abc-def"), None);
    }
}
