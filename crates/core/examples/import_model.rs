//! Импорт моделей из локальных файлов в каталог моделей приложения (проверка по размеру и SHA-256
//! каталога; копирование без сети). Нужен для отладки, пока в приложении нет экрана «Модели» (MVP-024+).
//!
//! `cargo run -p tolk-core --release --example import_model -- <каталог моделей> <файл>...`
//!
//! На macOS каталог моделей приложения: `~/Library/Application Support/kg.devcats.tolk/models`.
use std::path::PathBuf;

use tolk_core::download::{import_from_file, CancelToken};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next().map(PathBuf::from) else {
        eprintln!("Использование: import_model <каталог моделей> <файл>...");
        std::process::exit(2);
    };
    let cancel = CancelToken::new();
    let mut failed = false;
    for file in args {
        match import_from_file(file.as_ref(), &dir, &cancel) {
            Ok(entry) => println!("импортирована {} ({})", entry.id, file),
            Err(e) => {
                failed = true;
                println!("ошибка {file}: {e}");
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
