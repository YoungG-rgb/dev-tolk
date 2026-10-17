//! Ядро dev-tolk, не зависящее от Tauri: раскладка каталога данных, SQLite-хранилище, задания.
//! Тонкий слой команд в `app` вызывает эти модули (TECHNICAL_DECISIONS §2).

pub mod db;
pub mod error;
pub mod jobs;
pub mod layout;

pub use error::{Result, StoreError};
