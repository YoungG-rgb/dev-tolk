//! Очередь заданий: машина состояний и атомарные переходы в базе (TD-10).
//!
//! ```text
//! QUEUED ──► PROCESSING ──► COMPLETED
//!    │            ├────────► FAILED ──┐
//!    └──► CANCELLED ◄────────┘        │ повтор
//!             └──────────────────────►┴──► QUEUED
//! ```
//!
//! Разрешено: QUEUED→PROCESSING, QUEUED→CANCELLED, PROCESSING→COMPLETED|FAILED|CANCELLED,
//! FAILED→QUEUED, CANCELLED→QUEUED. Всё остальное запрещено, COMPLETED — конечное состояние.
//!
//! Каждый переход — один условный `UPDATE ... WHERE status IN (...)`: двойной клик, гонка отмены и
//! завершения решаются в базе, а не проверкой «прочитал, потом записал». Один исполнитель
//! гарантируется самим запросом захвата: задание не берётся, пока есть другое PROCESSING.

use std::time::{Duration, Instant};

use rusqlite::Connection;

use crate::error::{Result, StoreError};
use crate::jobs::{self, now_ms, Job, JobStatus};

/// Код ошибки для заданий, прерванных закрытием или падением приложения.
pub const ERROR_INTERRUPTED: &str = "INTERRUPTED";
pub const MESSAGE_INTERRUPTED: &str = "Обработка прервана";

/// Разрешён ли переход между статусами.
pub fn can_transition(from: JobStatus, to: JobStatus) -> bool {
    use JobStatus::*;
    matches!(
        (from, to),
        (Queued, Processing)
            | (Queued, Cancelled)
            | (Processing, Completed)
            | (Processing, Failed)
            | (Processing, Cancelled)
            | (Failed, Queued)
            | (Cancelled, Queued)
    )
}

/// Исходные статусы, из которых разрешён переход в `to`.
fn sources(to: JobStatus) -> Vec<JobStatus> {
    use JobStatus::*;
    [Queued, Processing, Completed, Failed, Cancelled]
        .into_iter()
        .filter(|from| can_transition(*from, to))
        .collect()
}

fn status_list(statuses: &[JobStatus]) -> String {
    statuses
        .iter()
        .map(|s| format!("'{}'", s.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Ошибка перехода: задание не найдено или находится в статусе, из которого переход запрещён.
fn transition_error(conn: &Connection, id: &str, to: JobStatus) -> StoreError {
    match jobs::get(conn, id) {
        Ok(Some(job)) => StoreError::InvalidTransition {
            id: id.to_string(),
            from: job.status.as_str(),
            to: to.as_str(),
        },
        Ok(None) => StoreError::JobNotFound(id.to_string()),
        Err(e) => e,
    }
}

/// Условный переход в `to`. `set` — дополнительные присваивания (`started_at = ?2`), `extra` — их параметры.
fn transition(
    conn: &Connection,
    id: &str,
    to: JobStatus,
    set: &str,
    extra: &[&dyn rusqlite::ToSql],
) -> Result<Job> {
    let sql = format!(
        "UPDATE jobs SET status = '{}', updated_at = ?1{} WHERE id = ?2 AND status IN ({})",
        to.as_str(),
        if set.is_empty() {
            String::new()
        } else {
            format!(", {set}")
        },
        status_list(&sources(to))
    );
    let now = now_ms();
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now, &id];
    params.extend_from_slice(extra);
    if conn.execute(&sql, params.as_slice())? == 0 {
        return Err(transition_error(conn, id, to));
    }
    jobs::get(conn, id)?.ok_or_else(|| StoreError::JobNotFound(id.to_string()))
}

/// QUEUED → PROCESSING для конкретного задания. Ошибка, если оно не в очереди или уже идёт
/// другое задание (один исполнитель).
pub fn start(conn: &Connection, id: &str) -> Result<Job> {
    let now = now_ms();
    let changed = conn.execute(
        "UPDATE jobs SET status = 'PROCESSING', updated_at = ?1, started_at = ?1, finished_at = NULL,
                progress = 0, attempts = attempts + 1, error_code = NULL, error_message = NULL
         WHERE id = ?2 AND status = 'QUEUED'
           AND NOT EXISTS (SELECT 1 FROM jobs WHERE status = 'PROCESSING')",
        rusqlite::params![now, id],
    )?;
    if changed == 0 {
        return Err(transition_error(conn, id, JobStatus::Processing));
    }
    jobs::get(conn, id)?.ok_or_else(|| StoreError::JobNotFound(id.to_string()))
}

/// Берёт самое раннее задание из очереди и переводит его в PROCESSING.
/// `None`, если очередь пуста или другое задание уже обрабатывается.
pub fn claim_next(conn: &Connection) -> Result<Option<Job>> {
    let Some(id): Option<String> = conn
        .query_row(
            "SELECT id FROM jobs WHERE status = 'QUEUED' ORDER BY rowid LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|e| {
            if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                Ok(None)
            } else {
                Err(e)
            }
        })?
    else {
        return Ok(None);
    };
    match start(conn, &id) {
        Ok(job) => Ok(Some(job)),
        // Занято другим заданием или задание только что отменили: брать нечего.
        Err(StoreError::InvalidTransition { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// PROCESSING → COMPLETED.
pub fn complete(conn: &Connection, id: &str) -> Result<Job> {
    let now = now_ms();
    transition(
        conn,
        id,
        JobStatus::Completed,
        "progress = 100, finished_at = ?3, error_code = NULL, error_message = NULL",
        &[&now],
    )
}

/// PROCESSING → FAILED с кодом и сообщением для пользователя.
pub fn fail(conn: &Connection, id: &str, code: &str, message: &str) -> Result<Job> {
    let now = now_ms();
    transition(
        conn,
        id,
        JobStatus::Failed,
        "finished_at = ?3, error_code = ?4, error_message = ?5",
        &[&now, &code, &message],
    )
}

/// QUEUED или PROCESSING → CANCELLED. Для PROCESSING вызывается после завершения процесса движка.
pub fn cancel(conn: &Connection, id: &str) -> Result<Job> {
    let now = now_ms();
    transition(conn, id, JobStatus::Cancelled, "finished_at = ?3", &[&now])
}

/// FAILED или CANCELLED → QUEUED. Счётчик попыток сохраняется, ошибка и прогресс сбрасываются.
pub fn retry(conn: &Connection, id: &str) -> Result<Job> {
    transition(
        conn,
        id,
        JobStatus::Queued,
        "progress = 0, started_at = NULL, finished_at = NULL, error_code = NULL, error_message = NULL",
        &[],
    )
}

/// Записывает прогресс идущего задания (0–100). Для остальных статусов ничего не меняет и возвращает `false`.
pub fn set_progress(conn: &Connection, id: &str, percent: u8) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE jobs SET progress = ?1, updated_at = ?2 WHERE id = ?3 AND status = 'PROCESSING'",
        rusqlite::params![percent.min(100), now_ms(), id],
    )?;
    Ok(changed > 0)
}

/// Восстановление при старте: всё, что осталось в PROCESSING, помечается FAILED («Обработка прервана»).
/// Автоматически не перезапускается: если причиной был нехватка памяти, перезапуск зациклил бы сбой.
pub fn recover_interrupted(conn: &Connection) -> Result<usize> {
    let now = now_ms();
    Ok(conn.execute(
        "UPDATE jobs SET status = 'FAILED', updated_at = ?1, finished_at = ?1, error_code = ?2, error_message = ?3
         WHERE status = 'PROCESSING'",
        rusqlite::params![now, ERROR_INTERRUPTED, MESSAGE_INTERRUPTED],
    )?)
}

/// Ограничитель записи прогресса в базу: не чаще раза в `interval`, но 100% — всегда.
/// Время передаётся снаружи, чтобы поведение проверялось без ожидания.
#[derive(Debug)]
pub struct ProgressThrottle {
    interval: Duration,
    last: Option<Instant>,
    last_percent: Option<u8>,
}

impl ProgressThrottle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
            last_percent: None,
        }
    }

    /// Нужно ли записать этот прогресс в базу.
    pub fn should_write(&mut self, now: Instant, percent: u8) -> bool {
        let due = match self.last {
            None => true,
            Some(last) => now.duration_since(last) >= self.interval,
        };
        let changed = self.last_percent != Some(percent);
        if changed && (due || percent >= 100) {
            self.last = Some(now);
            self.last_percent = Some(percent);
            return true;
        }
        false
    }
}
