//! Раскладка каталога данных (TECHNICAL_DECISIONS §2):
//!
//! ```text
//! <data>/
//!   tolk.db          SQLite: задания, настройки
//!   models/          установленные модели; *.part — незавершённые загрузки
//!   jobs/<job_id>/   source.<ext>, original.json, transcript.json
//!   tmp/             временные файлы, очищается при старте
//!   logs/            журналы приложения и движка
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct DataLayout {
    root: PathBuf,
}

impl DataLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Создаёт каталог данных и подкаталоги (повторный вызов безопасен).
    pub fn ensure(&self) -> Result<()> {
        for dir in [
            self.models_dir(),
            self.jobs_dir(),
            self.tmp_dir(),
            self.logs_dir(),
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("tolk.db")
    }

    pub fn models_dir(&self) -> PathBuf {
        self.root.join("models")
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.root.join("jobs")
    }

    pub fn job_dir(&self, job_id: &str) -> PathBuf {
        self.jobs_dir().join(job_id)
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
}
