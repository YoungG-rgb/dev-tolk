//! Команды очереди для интерфейса (MVP-016) и временная отладочная постановка файла (MVP-013).
//! `debug_enqueue` и `debug_models` удаляются в MVP-018/MVP-019, когда появятся импорт и экран очереди.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tolk_core::executor::{Executor, JobEvent};
use tolk_core::import::ImportEvent;
use tolk_core::jobs::{self, Job};
use tolk_core::layout::DataLayout;
use tolk_core::model_manager;
use tolk_core::models::SystemInfo;
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugModel {
    id: String,
    title: String,
    installed: bool,
}

#[tauri::command]
pub fn debug_models(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
) -> Result<Vec<DebugModel>, String> {
    let layout = DataLayout::new(app.path().app_local_data_dir().map_err(|e| e.to_string())?);
    let list = model_manager::list(&store.db, &layout.models_dir(), &SystemInfo::detect())
        .map_err(|e| user_error(&e))?;
    Ok(list
        .into_iter()
        .map(|s| DebugModel {
            id: s.entry.id.clone(),
            title: s.entry.title.clone(),
            installed: s.installed,
        })
        .collect())
}
