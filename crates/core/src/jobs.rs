//! Задания: статусы и минимальный доступ к таблице `jobs`.
//! Машина состояний, захват и восстановление очереди — в MVP-016.

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Row};

use crate::error::{Result, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Processing,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "QUEUED",
            JobStatus::Processing => "PROCESSING",
            JobStatus::Completed => "COMPLETED",
            JobStatus::Failed => "FAILED",
            JobStatus::Cancelled => "CANCELLED",
        }
    }
}

impl FromStr for JobStatus {
    type Err = StoreError;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "QUEUED" => JobStatus::Queued,
            "PROCESSING" => JobStatus::Processing,
            "COMPLETED" => JobStatus::Completed,
            "FAILED" => JobStatus::Failed,
            "CANCELLED" => JobStatus::Cancelled,
            other => return Err(StoreError::UnknownStatus(other.to_string())),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct NewJob {
    pub source_name: String,
    pub audio_path: Option<String>,
    pub duration_ms: Option<i64>,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub source_name: String,
    pub audio_path: Option<String>,
    pub duration_ms: Option<i64>,
    pub status: JobStatus,
    pub progress: u8,
    pub model_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attempts: u32,
    pub has_edits: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

pub(crate) const COLUMNS: &str = "id, source_name, audio_path, duration_ms, status, progress, model_id, error_code, \
                       error_message, attempts, has_edits, created_at, updated_at, started_at, finished_at";

pub(crate) fn from_row(row: &Row<'_>) -> rusqlite::Result<(Job, String)> {
    let status: String = row.get(4)?;
    let job = Job {
        id: row.get(0)?,
        source_name: row.get(1)?,
        audio_path: row.get(2)?,
        duration_ms: row.get(3)?,
        status: JobStatus::Queued,
        progress: row.get(5)?,
        model_id: row.get(6)?,
        error_code: row.get(7)?,
        error_message: row.get(8)?,
        attempts: row.get(9)?,
        has_edits: row.get::<_, i64>(10)? != 0,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        started_at: row.get(13)?,
        finished_at: row.get(14)?,
    };
    Ok((job, status))
}

/// Создаёт задание в статусе QUEUED.
pub fn insert(conn: &Connection, new: &NewJob) -> Result<Job> {
    insert_with_id(conn, &uuid::Uuid::new_v4().to_string(), new)
}

/// То же с заранее известным идентификатором: импорт сначала кладёт файлы в `jobs/<id>/`, и только
/// потом создаёт запись, чтобы исполнитель не взял задание без аудио.
pub fn insert_with_id(conn: &Connection, id: &str, new: &NewJob) -> Result<Job> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO jobs (id, source_name, audio_path, duration_ms, model_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        rusqlite::params![id, new.source_name, new.audio_path, new.duration_ms, new.model_id, now],
    )?;
    get(conn, id)?.ok_or(StoreError::Db(rusqlite::Error::QueryReturnedNoRows))
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Job>> {
    let found = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1"),
            [id],
            from_row,
        )
        .optional()?;
    found
        .map(|(mut job, status)| {
            job.status = status.parse()?;
            Ok(job)
        })
        .transpose()
}

/// Статус самого свежего задания с тем же именем исходного файла и той же длительностью (для
/// предупреждения «уже добавлено»). Файлы не сравниваются: имя и длительность — признак, а не доказательство.
pub fn find_similar(
    conn: &Connection,
    source_name: &str,
    duration_ms: Option<i64>,
) -> Result<Option<JobStatus>> {
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM jobs WHERE source_name = ?1 AND duration_ms IS ?2 ORDER BY created_at DESC, rowid DESC LIMIT 1",
            rusqlite::params![source_name, duration_ms],
            |r| r.get(0),
        )
        .optional()?;
    status.map(|s| s.parse()).transpose()
}

/// Все задания, новые сверху.
pub fn list(conn: &Connection) -> Result<Vec<Job>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM jobs ORDER BY created_at DESC, rowid DESC"
    ))?;
    let rows = stmt
        .query_map([], from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(mut job, status)| {
            job.status = status.parse()?;
            Ok(job)
        })
        .collect()
}

/// Идентификаторы всех заданий (для поиска «осиротевших» каталогов).
pub fn all_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM jobs")?;
    let ids = stmt
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ids)
}

/// Удаляет запись о задании. Возвращает, была ли такая запись.
pub fn delete(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn.execute("DELETE FROM jobs WHERE id = ?1", [id])? > 0)
}

/// Удаляет запись только у завершённого задания (COMPLETED, FAILED, CANCELLED): условный `DELETE`, поэтому
/// гонка с повтором или захватом исполнителем решается в базе. `false` — задания нет.
/// Ошибка `InvalidTransition`, если задание в очереди или в обработке.
pub fn delete_finished(conn: &Connection, id: &str) -> Result<bool> {
    let deleted = conn.execute(
        "DELETE FROM jobs WHERE id = ?1 AND status IN ('COMPLETED', 'FAILED', 'CANCELLED')",
        [id],
    )?;
    if deleted > 0 {
        return Ok(true);
    }
    match get(conn, id)? {
        None => Ok(false),
        Some(job) => Err(StoreError::InvalidTransition {
            id: id.to_string(),
            from: job.status.as_str(),
            to: "DELETED",
        }),
    }
}

/// Отмечает, отличается ли рабочая копия расшифровки от оригинала.
pub fn set_has_edits(conn: &Connection, id: &str, has_edits: bool) -> Result<()> {
    let changed = conn.execute(
        "UPDATE jobs SET has_edits = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![has_edits as i64, now_ms(), id],
    )?;
    if changed == 0 {
        return Err(StoreError::JobNotFound(id.to_string()));
    }
    Ok(())
}
