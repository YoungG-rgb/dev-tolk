//! SQLite: соединение, настройки надёжности и миграции через `PRAGMA user_version` (TD-08).
//!
//! В базе только метаданные заданий и настройки. Аудио и тексты транскриптов — файлами в `jobs/<id>/`.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OpenFlags};

use crate::error::{Result, StoreError};

/// Одна миграция схемы. Версии идут подряд с 1; применённая миграция не редактируется,
/// изменения оформляются новой версией.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("../migrations/001_init.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("../migrations/002_glossary.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("../migrations/003_metrics.sql"),
    },
];

/// Применяет недостающие миграции. Каждая выполняется в отдельной транзакции вместе с повышением
/// `user_version`: при сбое на середине база остаётся на прежней версии. Возвращает итоговую версию.
pub fn migrate(conn: &mut Connection, migrations: &[Migration]) -> Result<u32> {
    for (i, m) in migrations.iter().enumerate() {
        if m.version as usize != i + 1 {
            return Err(StoreError::BadMigrations(
                "версии должны идти подряд, начиная с 1",
            ));
        }
    }
    let latest = migrations.last().map_or(0, |m| m.version);
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current > latest {
        return Err(StoreError::SchemaTooNew {
            found: current,
            supported: latest,
        });
    }
    for m in migrations.iter().filter(|m| m.version > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
    }
    Ok(latest)
}

fn configure(conn: &Connection) -> Result<()> {
    // journal_mode возвращает строку, поэтому запрос, а не execute.
    let _mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(())
}

/// Единственное соединение под `Mutex`: запись в приложении одна и короткая (TD-08).
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Открывает (или создаёт) базу и приводит схему к актуальной версии.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, MIGRATIONS)
    }

    pub fn open_with(path: &Path, migrations: &[Migration]) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut conn = Connection::open_with_flags(path, flags)?;
        configure(&conn)?;
        migrate(&mut conn, migrations)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        // Паника в другом потоке во время запроса не делает соединение непригодным: транзакции откатываются.
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn()
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    /// Значение настройки.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}
