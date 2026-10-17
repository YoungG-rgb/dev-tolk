//! Ядро dev-tolk, не зависящее от Tauri: раскладка каталога данных, SQLite-хранилище, задания.
//! Тонкий слой команд в `app` вызывает эти модули (TECHNICAL_DECISIONS §2).

pub mod db;
pub mod error;
pub mod export;
pub mod files;
pub mod glossary;
pub mod jobs;
pub mod layout;
pub mod queue;
pub mod store;
pub mod transcript;

pub use error::{Result, StoreError};
