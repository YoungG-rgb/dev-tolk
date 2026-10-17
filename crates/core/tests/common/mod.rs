//! Минимальный HTTP-сервер для тестов загрузчика: поддерживает `Range`, редиректы и типичные сбои.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tolk_core::models::{ModelEntry, ModelKind};

#[derive(Debug, Clone)]
pub struct Hit {
    pub path: String,
    pub range: Option<String>,
}

pub struct Server {
    pub addr: SocketAddr,
    pub hits: Arc<Mutex<Vec<Hit>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub fn hit_count(&self) -> usize {
        self.hits.lock().unwrap().len()
    }

    pub fn ranges(&self) -> Vec<String> {
        self.hits
            .lock()
            .unwrap()
            .iter()
            .filter_map(|h| h.range.clone())
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr); // разбудить accept
    }
}

/// Тестовые «модель» и её запись каталога.
pub fn fake_model(len: usize) -> (Vec<u8>, ModelEntry) {
    let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
    let entry = ModelEntry {
        id: "test-model".into(),
        kind: ModelKind::Asr,
        title: "Тестовая модель".into(),
        engine: "whisper.cpp".into(),
        source: "тест".into(),
        repo: "test/test".into(),
        revision: "0".repeat(40),
        file: "ggml-test.bin".into(),
        url: String::new(),
        size: len as u64,
        sha256: hex(&Sha256::digest(&data)),
        license: "MIT".into(),
        ram_estimate_mb: None,
        note: None,
    };
    (data, entry)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn start(data: Vec<u8>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let flaky_counter = Arc::new(AtomicUsize::new(0));
    let data = Arc::new(data);
    let (h, s) = (Arc::clone(&hits), Arc::clone(&stop));
    thread::spawn(move || {
        for conn in listener.incoming() {
            if s.load(Ordering::SeqCst) {
                break;
            }
            let Ok(conn) = conn else { continue };
            let (data, hits, flaky) = (
                Arc::clone(&data),
                Arc::clone(&h),
                Arc::clone(&flaky_counter),
            );
            thread::spawn(move || {
                let _ = handle(conn, &data, &hits, &flaky, addr);
            });
        }
    });
    Server { addr, hits, stop }
}

fn read_request(conn: &mut TcpStream) -> Option<(String, Option<String>, String)> {
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        if conn.read(&mut byte).ok()? == 0 {
            return None;
        }
        raw.push(byte[0]);
        if raw.len() > 16 * 1024 {
            return None;
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let first = text.lines().next()?.to_string();
    let target = first.split_whitespace().nth(1)?.to_string();
    let range = text.lines().find_map(|l| {
        l.to_lowercase()
            .starts_with("range:")
            .then(|| l[6..].trim().to_string())
    });
    Some((target, range, text))
}

fn parse_range(range: &str, len: usize) -> Option<(usize, usize)> {
    let spec = range.strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let start: usize = a.parse().ok()?;
    let end = if b.is_empty() {
        len.checked_sub(1)?
    } else {
        b.parse::<usize>().ok()?.min(len.checked_sub(1)?)
    };
    (start <= end).then_some((start, end))
}

fn handle(
    mut conn: TcpStream,
    data: &[u8],
    hits: &Mutex<Vec<Hit>>,
    flaky: &AtomicUsize,
    addr: SocketAddr,
) -> std::io::Result<()> {
    let Some((target, range, _)) = read_request(&mut conn) else {
        return Ok(());
    };
    let path = target
        .split("://")
        .last()
        .map(|r| r.find('/').map_or("/", |i| &r[i..]))
        .unwrap_or("/")
        .to_string();
    hits.lock().unwrap().push(Hit {
        path: path.clone(),
        range: range.clone(),
    });
    let simple = |conn: &mut TcpStream, status: &str, extra: &str| -> std::io::Result<()> {
        write!(
            conn,
            "HTTP/1.1 {status}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
        )
    };
    match path.as_str() {
        "/redirect" => simple(
            &mut conn,
            "302 Found",
            &format!("Location: http://{addr}/ok\r\n"),
        ),
        "/offsite" => simple(
            &mut conn,
            "302 Found",
            "Location: http://evil.invalid:1/x\r\n",
        ),
        "/forbidden" => simple(&mut conn, "403 Forbidden", ""),
        "/proxyauth" => simple(&mut conn, "407 Proxy Authentication Required", ""),
        "/notfound" => simple(&mut conn, "404 Not Found", ""),
        "/busy" => simple(&mut conn, "503 Service Unavailable", ""),
        "/ratelimit" => simple(&mut conn, "429 Too Many Requests", ""),
        "/norange" => {
            write!(
                conn,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                data.len()
            )?;
            conn.write_all(data)
        }
        "/badrange" => {
            let (s, e) = range
                .as_deref()
                .and_then(|r| parse_range(r, data.len()))
                .unwrap_or((0, data.len() - 1));
            let body = &data[s..=e];
            write!(conn, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", s + 1, e + 1, data.len(), body.len())?;
            conn.write_all(body)
        }
        "/stall" => {
            write!(conn, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-9/{}\r\nContent-Length: 10\r\nConnection: close\r\n\r\n", data.len())?;
            conn.flush()?;
            thread::sleep(Duration::from_secs(5));
            Ok(())
        }
        _ => {
            // /ok, /flaky, /corrupt, /short — отдают данные с поддержкой Range.
            let mut body_src: Vec<u8> = data.to_vec();
            if path == "/corrupt" {
                for b in body_src.iter_mut().step_by(1000) {
                    *b = b.wrapping_add(1);
                }
            }
            if path == "/short" {
                body_src.truncate(data.len() - 10);
            }
            let len = body_src.len();
            let Some(r) = range.as_deref() else {
                write!(
                    conn,
                    "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
                )?;
                return conn.write_all(&body_src);
            };
            let Some((s, e)) = parse_range(r, len) else {
                return simple(
                    &mut conn,
                    "416 Range Not Satisfiable",
                    &format!("Content-Range: bytes */{len}\r\n"),
                );
            };
            let body = &body_src[s..=e];
            write!(conn, "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {s}-{e}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
            if path == "/flaky" && flaky.fetch_add(1, Ordering::SeqCst) == 1 {
                // Второй запрос обрывается на середине тела: «обрыв сети».
                conn.write_all(&body[..body.len() / 2])?;
                conn.flush()?;
                return Ok(());
            }
            conn.write_all(body)
        }
    }
}
