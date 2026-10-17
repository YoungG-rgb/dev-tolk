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
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_ms();
    conn.execute(
        "INSERT INTO jobs (id, source_name, audio_path, duration_ms, model_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        rusqlite::params![id, new.source_name, new.audio_path, new.duration_ms, new.model_id, now],
    )?;
    get(conn, &id)?.ok_or(StoreError::Db(rusqlite::Error::QueryReturnedNoRows))
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
