//! Ядро dev-tolk, не зависящее от Tauri: раскладка каталога данных, SQLite-хранилище, задания.
//! Тонкий слой команд в `app` вызывает эти модули (TECHNICAL_DECISIONS §2).

pub mod db;
pub mod diagnostics;
pub mod disk;
pub mod download;
pub mod engine;
pub mod error;
pub mod executor;
pub mod export;
pub mod files;
pub mod glossary;
pub mod import;
pub mod jobs;
pub mod layout;
pub mod metrics;
pub mod model_manager;
pub mod models;
pub mod power;
pub mod queue;
pub mod store;
pub mod transcript;
pub mod user_errors;

pub use error::{Result, StoreError};
