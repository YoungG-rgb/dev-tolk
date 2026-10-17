//! Файловое хранилище заданий: `jobs/<id>/` (TECHNICAL_DECISIONS §2, TD-08, TD-09).
//!
//! Правила надёжности:
//! - итоговый файл появляется только целиком: данные пишутся во временный файл в `tmp/`
//!   (тот же том, что и `jobs/`), сбрасываются на диск (`sync_all`) и переименовываются;
//! - исходные файлы пользователя только читаются, никогда не изменяются и не удаляются;
//! - при занятом файле (антивирус, Word) операции повторяются и заканчиваются понятной ошибкой;
//! - при старте `cleanup_tmp` и `cleanup_orphans` убирают следы прерванных операций.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use tempfile::{NamedTempFile, TempPath};

use crate::error::{Result, StoreError};
use crate::layout::DataLayout;

/// Сколько раз повторяется операция над занятым файлом и пауза между попытками.
const BUSY_RETRIES: u32 = 5;
const BUSY_PAUSE: Duration = Duration::from_millis(100);

/// Файловые операции над каталогами заданий.
#[derive(Debug, Clone)]
pub struct JobFiles {
    layout: DataLayout,
}

impl JobFiles {
    pub fn new(layout: DataLayout) -> Self {
        Self { layout }
    }

    pub fn layout(&self) -> &DataLayout {
        &self.layout
    }

    /// Каталог задания; создаётся при необходимости.
    pub fn ensure_job_dir(&self, job_id: &str) -> Result<PathBuf> {
        let dir = self.job_dir(job_id)?;
        fs::create_dir_all(&dir).map_err(map_write_error)?;
        Ok(dir)
    }

    fn job_dir(&self, job_id: &str) -> Result<PathBuf> {
        validate_id(job_id)?;
        Ok(self.layout.job_dir(job_id))
    }

    fn job_file(&self, job_id: &str, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.job_dir(job_id)?.join(name))
    }

    /// Копирует аудио пользователя в `jobs/<id>/<name>` (например, `source.mp3`). Возвращает размер в байтах.
    /// Исходный файл только читается.
    pub fn copy_audio(&self, job_id: &str, source: &Path, name: &str) -> Result<u64> {
        self.copy_audio_with_hook(job_id, source, name, || {})
    }

    /// То же, но `before_commit` вызывается после копирования и сброса на диск, перед переименованием.
    /// Нужен тестам прерывания процесса.
    #[doc(hidden)]
    pub fn copy_audio_with_hook(
        &self,
        job_id: &str,
        source: &Path,
        name: &str,
        before_commit: impl FnOnce(),
    ) -> Result<u64> {
        let dest = self.job_file(job_id, name)?;
        self.ensure_job_dir(job_id)?;
        let mut input = File::open(source)?;
        let tmp = self.new_temp()?;
        let mut out = BufWriter::with_capacity(1 << 20, tmp.reopen().map_err(map_write_error)?);
        let bytes = io::copy(&mut input, &mut out).map_err(map_write_error)?;
        out.flush().map_err(map_write_error)?;
        out.get_ref().sync_all().map_err(map_write_error)?;
        drop(out);
        before_commit();
        commit(tmp, &dest)?;
        Ok(bytes)
    }

    /// Атомарно записывает файл в `jobs/<id>/<name>` (например, `transcript.json`).
    /// Старая версия остаётся целой, пока новая не записана полностью.
    pub fn write_atomic(&self, job_id: &str, name: &str, data: &[u8]) -> Result<()> {
        self.write_atomic_with_hook(job_id, name, data, || {})
    }

    /// То же, но `before_commit` вызывается после записи и сброса на диск, перед переименованием.
    /// Нужен тестам прерывания процесса.
    #[doc(hidden)]
    pub fn write_atomic_with_hook(
        &self,
        job_id: &str,
        name: &str,
        data: &[u8],
        before_commit: impl FnOnce(),
    ) -> Result<()> {
        let dest = self.job_file(job_id, name)?;
        self.ensure_job_dir(job_id)?;
        let tmp = self.new_temp()?;
        let mut file = tmp.reopen().map_err(map_write_error)?;
        file.write_all(data).map_err(map_write_error)?;
        file.sync_all().map_err(map_write_error)?;
        drop(file);
        before_commit();
        commit(tmp, &dest)
    }

    /// Читает файл задания; `None`, если его нет.
    pub fn read(&self, job_id: &str, name: &str) -> Result<Option<Vec<u8>>> {
        match fs::read(self.job_file(job_id, name)?) {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Удаляет каталог задания со всем содержимым. Отсутствующий каталог — не ошибка.
    pub fn delete_job_dir(&self, job_id: &str) -> Result<()> {
        let dir = self.job_dir(job_id)?;
        retry_busy(&dir, || match fs::remove_dir_all(&dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        })
    }

    /// Занятое заданием место на диске, байт.
    pub fn job_size(&self, job_id: &str) -> Result<u64> {
        Ok(dir_size(&self.job_dir(job_id)?))
    }

    /// Всё место, занятое заданиями, байт.
    pub fn total_size(&self) -> u64 {
        dir_size(&self.layout.jobs_dir())
    }

    /// Очищает `tmp/` (остатки прерванных копирований и записей). Вызывать при старте.
    pub fn cleanup_tmp(&self) -> Result<usize> {
        let mut removed = 0;
        let dir = self.layout.tmp_dir();
        if !dir.exists() {
            return Ok(0);
        }
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let result = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            if result.is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Удаляет каталоги заданий, которых нет в базе (`known_ids`). Вызывать при старте.
    /// Возвращает идентификаторы удалённых каталогов.
    pub fn cleanup_orphans(&self, known_ids: &HashSet<String>) -> Result<Vec<String>> {
        let mut removed = Vec::new();
        let dir = self.layout.jobs_dir();
        if !dir.exists() {
            return Ok(removed);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !known_ids.contains(&name) && fs::remove_dir_all(entry.path()).is_ok() {
                removed.push(name);
            }
        }
        removed.sort();
        Ok(removed)
    }

    fn new_temp(&self) -> Result<NamedTempFile> {
        let tmp_dir = self.layout.tmp_dir();
        fs::create_dir_all(&tmp_dir).map_err(map_write_error)?;
        tempfile::Builder::new()
            .prefix("w-")
            .suffix(".part")
            .tempfile_in(tmp_dir)
            .map_err(map_write_error)
    }
}

/// Переименовывает готовый временный файл в итоговый, с повторами для занятых файлов.
fn commit(tmp: NamedTempFile, dest: &Path) -> Result<()> {
    let mut path: TempPath = tmp.into_temp_path();
    let mut attempt = 0;
    loop {
        match path.persist(dest) {
            Ok(()) => {
                sync_parent_dir(dest);
                return Ok(());
            }
            Err(e) if is_busy(&e.error) && attempt < BUSY_RETRIES => {
                attempt += 1;
                path = e.path;
                sleep(BUSY_PAUSE);
            }
            Err(e) if is_busy(&e.error) => {
                return Err(StoreError::FileBusy(dest.display().to_string()))
            }
            Err(e) => return Err(map_write_error(e.error)),
        }
    }
}

/// Повторяет операцию над каталогом, пока он занят.
fn retry_busy(path: &Path, mut op: impl FnMut() -> io::Result<()>) -> Result<()> {
    let mut attempt = 0;
    loop {
        match op() {
            Ok(()) => return Ok(()),
            Err(e) if is_busy(&e) && attempt < BUSY_RETRIES => {
                attempt += 1;
                sleep(BUSY_PAUSE);
            }
            Err(e) if is_busy(&e) => return Err(StoreError::FileBusy(path.display().to_string())),
            Err(e) => return Err(e.into()),
        }
    }
}

/// Файл занят другим процессом. На Windows это отказ в доступе (5) и нарушение совместного
/// доступа (32, 33); на остальных системах такого состояния нет.
fn is_busy(err: &io::Error) -> bool {
    cfg!(windows) && matches!(err.raw_os_error(), Some(5 | 32 | 33))
}

fn map_write_error(err: io::Error) -> StoreError {
    match err.kind() {
        io::ErrorKind::StorageFull => StoreError::NoSpace,
        _ => StoreError::Io(err),
    }
}

/// Сбрасывает каталог на диск, чтобы переименование пережило сбой питания (Unix; на Windows недоступно).
fn sync_parent_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Идентификатор задания — только символы UUID: защита от выхода за пределы `jobs/`.
fn validate_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if ok {
        Ok(())
    } else {
        Err(StoreError::InvalidId(id.chars().take(64).collect()))
    }
}

/// Имя файла внутри каталога задания: без разделителей путей и служебных имён.
fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0']);
    if ok {
        Ok(())
    } else {
        Err(StoreError::InvalidId(name.chars().take(64).collect()))
    }
}
