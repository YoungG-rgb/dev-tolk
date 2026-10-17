//! Диагностика сети для загрузки моделей: достижим ли адрес модели с этого компьютера
//! (системный прокси, сертификаты, блокировка доменов). Скачивает один байт, ничего не сохраняет.
//!
//! `cargo run -p tolk-core --example net_check` — проверяет адреса всех моделей каталога;
//! `cargo run -p tolk-core --example net_check -- <id модели>` — одну модель.
use tolk_core::download::{check_connection, DownloadConfig};
use tolk_core::models::catalog;

fn main() {
    let only = std::env::args().nth(1);
    let cfg = DownloadConfig::production();
    for proxy_var in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "NO_PROXY",
    ] {
        if let Ok(v) = std::env::var(proxy_var) {
            println!("переменная окружения {proxy_var} задана: {v}");
        }
    }
    let mut failed = false;
    for m in catalog()
        .iter()
        .filter(|m| only.as_deref().is_none_or(|id| id == m.id))
    {
        match check_connection(&m.url, &cfg) {
            Ok(status) => println!("ок   {} (HTTP {status})", m.id),
            Err(e) => {
                failed = true;
                println!(
                    "ОШИБКА {}: {e}{}",
                    m.id,
                    if e.suggest_import() {
                        " — можно импортировать модель из файла"
                    } else {
                        ""
                    }
                );
            }
        }
    }
    std::process::exit(i32::from(failed));
}
