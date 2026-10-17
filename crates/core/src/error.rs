use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("ошибка базы данных: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("ошибка файловой системы: {0}")]
    Io(#[from] std::io::Error),
    /// База создана более новой версией приложения: открывать её нельзя, чтобы не повредить данные.
    #[error("база данных создана более новой версией приложения (версия схемы {found}, поддерживается {supported})")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("некорректный список миграций: {0}")]
    BadMigrations(&'static str),
    #[error("неизвестный статус задания: {0}")]
    UnknownStatus(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;
