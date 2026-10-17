//! Команды очереди и истории для интерфейса (MVP-016, MVP-019).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;
use tolk_core::executor::{Executor, JobEvent};
use tolk_core::export::group_paragraphs;
use tolk_core::import::ImportEvent;
use tolk_core::jobs::{self, Job};
use tolk_core::queue;
use tolk_core::store::Store;
use tolk_core::user_errors;

/// Всё, что приложение сообщает интерфейсу о заданиях и импорте (разные значения `kind`).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum AppEvent {
    Job(JobEvent),
    Import(ImportEvent),
}

/// Каналы, в которые рассылаются события (по одному на подписавшееся окно).
#[derive(Default)]
pub struct Subscribers(Mutex<Vec<Channel<AppEvent>>>);

impl Subscribers {
    pub fn publish(&self, event: AppEvent) {
        let mut channels = self.0.lock().unwrap_or_else(|e| e.into_inner());
        channels.retain(|c| c.send(event.clone()).is_ok());
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDto {
    id: String,
    source_name: String,
    status: String,
    duration_ms: Option<i64>,
    progress: u8,
    model_id: Option<String>,
    attempts: u32,
    error_code: Option<String>,
    error_message: Option<String>,
    error_action: Option<String>,
    created_at: i64,
    finished_at: Option<i64>,
}

impl From<Job> for JobDto {
    fn from(j: Job) -> Self {
        let error_action = j
            .error_code
            .as_deref()
            .map(|c| user_errors::from_engine(c, None).action);
        Self {
            id: j.id,
            source_name: j.source_name,
            status: j.status.as_str().to_string(),
            duration_ms: j.duration_ms,
            progress: j.progress,
            model_id: j.model_id,
            attempts: j.attempts,
            error_code: j.error_code,
            error_message: j.error_message,
            error_action,
            created_at: j.created_at,
            finished_at: j.finished_at,
        }
    }
}

fn user_error(e: &tolk_core::StoreError) -> String {
    let m = user_errors::from_store(e);
    log::warn!("Команда очереди не выполнена: {:?}", m.technical);
    m.message
}

/// Путь к сайдкару: рядом с исполняемым файлом приложения (в собранном `.app` это `Contents/MacOS`),
/// в разработке — `TOLK_ENGINE` или `target/{release,debug}`.
pub fn engine_exe() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "tolk-engine.exe"
    } else {
        "tolk-engine"
    };
    let beside = std::env::current_exe().ok()?.parent()?.join(name);
    if beside.is_file() {
        return Some(beside);
    }
    if let Some(p) = std::env::var_os("TOLK_ENGINE") {
        return Some(PathBuf::from(p));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    ["release", "debug"]
        .iter()
        .map(|p| root.join(p).join(name))
        .find(|p| p.is_file())
}

#[tauri::command]
pub fn jobs_list(store: State<'_, Arc<Store>>) -> Result<Vec<JobDto>, String> {
    let conn = store.db.conn();
    Ok(jobs::list(&conn)
        .map_err(|e| user_error(&e))?
        .into_iter()
        .map(JobDto::from)
        .collect())
}

#[tauri::command]
pub fn jobs_subscribe(subscribers: State<'_, Arc<Subscribers>>, on_event: Channel<AppEvent>) {
    subscribers
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(on_event);
}

#[tauri::command]
pub fn job_cancel(
    executor: State<'_, Arc<Executor>>,
    subscribers: State<'_, Arc<Subscribers>>,
    id: String,
) -> Result<JobDto, String> {
    let job = executor.cancel_job(&id).map_err(|e| user_error(&e))?;
    // Отмена задания в очереди завершается сразу; об идущем сообщит исполнитель после остановки движка.
    if job.status == jobs::JobStatus::Cancelled {
        subscribers.publish(AppEvent::Job(JobEvent::Finished {
            job_id: id,
            status: "CANCELLED".into(),
            error_code: None,
            error_message: None,
        }));
    }
    Ok(job.into())
}

#[tauri::command]
pub fn job_retry(
    store: State<'_, Arc<Store>>,
    executor: State<'_, Arc<Executor>>,
    id: String,
) -> Result<JobDto, String> {
    let job = {
        let conn = store.db.conn();
        queue::retry(&conn, &id).map_err(|e| user_error(&e))?
    };
    executor.notify();
    Ok(job.into())
}

/// Удаление завершённого задания вместе с файлами. Задание в очереди или в обработке сначала отменяют.
#[tauri::command]
pub fn job_delete(
    store: State<'_, Arc<Store>>,
    executor: State<'_, Arc<Executor>>,
    id: String,
) -> Result<(), String> {
    if executor.is_running(&id) {
        return Err(
            user_errors::from_store(&tolk_core::StoreError::InvalidTransition {
                id,
                from: "PROCESSING",
                to: "DELETED",
            })
            .message,
        );
    }
    store.delete_finished_job(&id).map_err(|e| user_error(&e))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paragraph {
    start_ms: u64,
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptView {
    title: String,
    model: String,
    duration_ms: u64,
    has_edits: bool,
    paragraphs: Vec<Paragraph>,
}

/// Текст готового задания для просмотра: рабочая копия, а при её отсутствии оригинал.
#[tauri::command]
pub fn job_transcript(store: State<'_, Arc<Store>>, id: String) -> Result<TranscriptView, String> {
    let job = {
        let conn = store.db.conn();
        jobs::get(&conn, &id)
            .map_err(|e| user_error(&e))?
            .ok_or_else(|| user_error(&tolk_core::StoreError::JobNotFound(id.clone())))?
    };
    let bytes = [("transcript.json"), ("original.json")]
        .iter()
        .find_map(|name| store.files.read(&id, name).transpose())
        .transpose()
        .map_err(|e| user_error(&e))?
        .ok_or("Файл с расшифровкой не найден.")?;
    let transcript =
        tolk_core::transcript::Transcript::from_json(&bytes).map_err(|e| user_error(&e))?;
    Ok(TranscriptView {
        title: job.source_name,
        model: transcript.model.clone(),
        duration_ms: transcript.duration_ms,
        has_edits: job.has_edits,
        paragraphs: group_paragraphs(&transcript)
            .into_iter()
            .map(|b| Paragraph {
                start_ms: b.start_ms,
                text: b.text,
            })
            .collect(),
    })
}
