//! Хранилище целиком: база данных и файлы заданий. Здесь согласуются операции над обоими.

use std::collections::HashSet;

use crate::db::Db;
use crate::error::Result;
use crate::files::JobFiles;
use crate::jobs;
use crate::layout::DataLayout;

/// Итоги очистки при старте.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StartupCleanup {
    /// Удалено временных файлов из `tmp/`.
    pub tmp_removed: usize,
    /// Каталоги заданий без записи в базе (остатки прерванного удаления или импорта).
    pub orphans_removed: Vec<String>,
}

pub struct Store {
    pub db: Db,
    pub files: JobFiles,
}

impl Store {
    /// Создаёт каталоги, открывает базу и убирает следы прерванных операций.
    pub fn open(layout: DataLayout) -> Result<(Self, StartupCleanup)> {
        layout.ensure()?;
        let db = Db::open(&layout.db_path())?;
        let store = Self {
            db,
            files: JobFiles::new(layout),
        };
        let cleanup = store.cleanup()?;
        Ok((store, cleanup))
    }

    fn cleanup(&self) -> Result<StartupCleanup> {
        let known: HashSet<String> = jobs::all_ids(&self.db.conn())?.into_iter().collect();
        Ok(StartupCleanup {
            tmp_removed: self.files.cleanup_tmp()?,
            orphans_removed: self.files.cleanup_orphans(&known)?,
        })
    }

    /// Удаляет задание: сначала запись в базе, затем файлы. При сбое между шагами остаётся каталог без записи,
    /// его уберёт очистка при старте; обратный порядок оставил бы запись без файлов.
    pub fn delete_job(&self, job_id: &str) -> Result<bool> {
        let existed = jobs::delete(&self.db.conn(), job_id)?;
        self.files.delete_job_dir(job_id)?;
        Ok(existed)
    }
}
