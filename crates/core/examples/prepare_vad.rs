//! Кладёт модель определения речи (Silero VAD, ~1 МБ) в каталог ресурсов приложения, чтобы она
//! устанавливалась из `.app` без сети (MVP-030, TD-13). Размер и SHA-256 проверяются по каталогу.
//!
//! `cargo run -p tolk-core --example prepare_vad -- <каталог ресурсов>`
//!
//! Порядок: файл уже на месте и верный → ничего; есть копия в `../dev-tolk-tools/models` → копируется
//! (через проверку); иначе загрузка с закреплённого адреса каталога (только домены из списка доверенных).
use std::path::{Path, PathBuf};

use tolk_core::download::{self, is_installed, CancelToken, DownloadConfig};
use tolk_core::models::{find, VAD_MODEL_ID};

fn main() {
    let Some(dest) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("Использование: prepare_vad <каталог ресурсов>");
        std::process::exit(2);
    };
    let entry = find(VAD_MODEL_ID).expect("модель определения речи есть в каталоге");
    std::fs::create_dir_all(&dest).expect("создать каталог ресурсов");
    if is_installed(entry, &dest) && check_sha(&dest.join(&entry.file), &entry.sha256) {
        println!("модель определения речи уже в {}", dest.display());
        return;
    }
    let local = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../dev-tolk-tools/models")
        .join(&entry.file);
    let cancel = CancelToken::new();
    let result = if local.is_file() {
        println!("копирование из {}", local.display());
        download::import_from_file(&local, &dest, &cancel).map(|_| ())
    } else {
        println!("загрузка {}", entry.url);
        download::download(
            entry,
            &dest,
            &DownloadConfig::production(),
            &cancel,
            &mut |_| {},
        )
        .map(|_| ())
    };
    if let Err(e) = result {
        eprintln!("не удалось подготовить модель определения речи: {e}");
        std::process::exit(1);
    }
    println!("готово: {}", dest.join(&entry.file).display());
}

fn check_sha(path: &Path, expected: &str) -> bool {
    use sha2::{Digest, Sha256};
    std::fs::read(path)
        .map(|b| {
            Sha256::digest(b)
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect::<String>()
                .eq_ignore_ascii_case(expected)
        })
        .unwrap_or(false)
}
