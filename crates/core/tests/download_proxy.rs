//! Прокси из переменных окружения: загрузчик должен использовать его и уважать NO_PROXY.
//! Отдельный тестовый бинарник: переменные окружения общие для процесса, тест один и последовательный.
//!
//! Прокси из настроек ОС (macOS SCDynamicStore, реестр Windows) здесь НЕ проверяется: для этого нужно
//! менять системные настройки, а в тестовой среде их нет. См. docs/TECHNICAL_DECISIONS.md, TD-13.

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use common::{fake_model, start};
use tolk_core::download::{download, CancelToken, DownloadConfig, DownloadError};

#[derive(Clone, Copy)]
enum Mode {
    Forward,
    Deny407,
}

struct Proxy {
    port: u16,
    seen: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// Простейший HTTP-прокси: принимает `GET http://host:port/path`, пересылает запрос на сервер и возвращает ответ.
fn start_proxy(mode: Mode) -> Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (s, st) = (Arc::clone(&seen), Arc::clone(&stop));
    thread::spawn(move || {
        for conn in listener.incoming() {
            if st.load(Ordering::SeqCst) {
                break;
            }
            let Ok(mut client) = conn else { continue };
            let s = Arc::clone(&s);
            thread::spawn(move || {
                let mut raw = Vec::new();
                let mut b = [0u8; 1];
                while !raw.ends_with(b"\r\n\r\n") {
                    if client.read(&mut b).unwrap_or(0) == 0 {
                        return;
                    }
                    raw.push(b[0]);
                }
                s.fetch_add(1, Ordering::SeqCst);
                if matches!(mode, Mode::Deny407) {
                    let _ = client.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    return;
                }
                let text = String::from_utf8_lossy(&raw).to_string();
                let first = text.lines().next().unwrap_or_default().to_string();
                let uri = first
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let Some(rest) = uri.strip_prefix("http://") else {
                    return;
                };
                let (host, path) = rest
                    .split_once('/')
                    .map(|(h, p)| (h.to_string(), format!("/{p}")))
                    .unwrap_or((rest.to_string(), "/".into()));
                let Ok(mut origin) = TcpStream::connect(&host) else {
                    return;
                };
                let rewritten = text.replacen(&uri, &path, 1);
                let _ = origin.write_all(rewritten.as_bytes());
                let mut buf = [0u8; 16 * 1024];
                while let Ok(n) = origin.read(&mut buf) {
                    if n == 0 || client.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    Proxy { port, seen, stop }
}

fn cfg() -> DownloadConfig {
    DownloadConfig {
        allowed_hosts: vec!["127.0.0.1".into()],
        https_only: false,
        system_proxy: true,
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(5),
        chunk_bytes: 64 * 1024,
    }
}

fn clear_proxy_env() {
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        std::env::remove_var(name);
    }
}

#[test]
fn environment_proxy_is_used_and_no_proxy_is_respected() {
    clear_proxy_env();
    let (data, mut entry) = fake_model(200_000);
    let origin = start(data.clone());
    entry.url = origin.url("/ok");

    // 1. Прокси не задан: прямое соединение.
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(Mode::Forward);
    download(&entry, dir.path(), &cfg(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(
        proxy.seen.load(Ordering::SeqCst),
        0,
        "без настройки прокси не используется"
    );

    // 2. HTTP_PROXY задан: все запросы идут через прокси, файл скачивается целиком.
    std::env::set_var("HTTP_PROXY", format!("http://127.0.0.1:{}", proxy.port));
    let dir = tempfile::tempdir().unwrap();
    let path = download(&entry, dir.path(), &cfg(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), data);
    let via_proxy = proxy.seen.load(Ordering::SeqCst);
    assert!(
        via_proxy >= 3,
        "запросы блоков прошли через прокси: {via_proxy}"
    );

    // 3. NO_PROXY исключает адрес: прокси обходится.
    std::env::set_var("NO_PROXY", "127.0.0.1");
    let dir = tempfile::tempdir().unwrap();
    download(&entry, dir.path(), &cfg(), &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(
        proxy.seen.load(Ordering::SeqCst),
        via_proxy,
        "NO_PROXY соблюдается"
    );
    std::env::remove_var("NO_PROXY");

    // 4. system_proxy = false: окружение игнорируется.
    let mut direct = cfg();
    direct.system_proxy = false;
    let dir = tempfile::tempdir().unwrap();
    download(
        &entry,
        dir.path(),
        &direct,
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(proxy.seen.load(Ordering::SeqCst), via_proxy);

    // 5. Прокси требует авторизацию: понятная ошибка с предложением импорта файла.
    let deny = start_proxy(Mode::Deny407);
    std::env::set_var("HTTP_PROXY", format!("http://127.0.0.1:{}", deny.port));
    let dir = tempfile::tempdir().unwrap();
    let err = download(&entry, dir.path(), &cfg(), &CancelToken::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, DownloadError::ProxyAuthRequired), "{err:?}");
    assert!(err.suggest_import());

    clear_proxy_env();
}
