//! Команды очереди и истории для интерфейса (MVP-016, MVP-019).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{Manager, State};
use tolk_core::editing::{self, EditView};
use tolk_core::executor::{Executor, JobEvent};
use tolk_core::import::ImportEvent;
use tolk_core::jobs::{self, Job};
use tolk_core::queue;
use tolk_core::store::Store;
use tolk_core::user_errors;

/// Сколько ждать ответа страницы на запрос сохранения при закрытии окна.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(6);

/// Всё, что приложение сообщает интерфейсу о заданиях и импорте (разные значения `kind`).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum AppEvent {
    Job(JobEvent),
    Import(ImportEvent),
    Request(EditorRequest),
    Model(crate::models_commands::ModelEvent),
}

/// Запросы приложения к странице.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EditorRequest {
    /// Сохранить несохранённые правки и подтвердить закрытие окна (`editor_close_now`).
    FlushAndClose,
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
    has_edits: bool,
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
            has_edits: j.has_edits,
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
pub struct SegmentDto {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditDto {
    title: String,
    model: String,
    duration_ms: u64,
    has_edits: bool,
    segments: Vec<SegmentDto>,
}

impl From<EditView> for EditDto {
    fn from(v: EditView) -> Self {
        Self {
            title: v.title,
            model: v.model,
            duration_ms: v.duration_ms,
            has_edits: v.has_edits,
            segments: v
                .segments
                .into_iter()
                .map(|s| SegmentDto {
                    start_ms: s.start_ms,
                    end_ms: s.end_ms,
                    text: s.text,
                })
                .collect(),
        }
    }
}

/// Расшифровка готового задания для просмотра и правки: рабочая копия, при её порче — оригинал.
#[tauri::command]
pub fn transcript_load(store: State<'_, Arc<Store>>, id: String) -> Result<EditDto, String> {
    editing::load(&store, &id)
        .map(Into::into)
        .map_err(|e| user_error(&e))
}

/// Сохраняет тексты сегментов (по одному на сегмент). Возвращает, отличается ли результат от оригинала.
#[tauri::command]
pub fn transcript_save(
    store: State<'_, Arc<Store>>,
    id: String,
    texts: Vec<String>,
) -> Result<bool, String> {
    editing::save_texts(&store, &id, &texts).map_err(|e| user_error(&e))
}

/// «Вернуть исходный текст».
#[tauri::command]
pub fn transcript_revert(store: State<'_, Arc<Store>>, id: String) -> Result<EditDto, String> {
    editing::revert(&store, &id)
        .map(Into::into)
        .map_err(|e| user_error(&e))
}

/// Состояние редактора для закрытия окна: есть ли несохранённые правки и ответила ли страница на запрос
/// сохранения. Без ответа страницы (она «зависла») окно закрывается само через `FLUSH_TIMEOUT`.
#[derive(Default)]
pub struct EditorState {
    dirty: AtomicBool,
    acked: AtomicBool,
}

impl EditorState {
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }
}

#[tauri::command]
pub fn editor_set_dirty(editor: State<'_, Arc<EditorState>>, dirty: bool) {
    editor.dirty.store(dirty, Ordering::SeqCst);
}

/// Страница получила запрос сохранить правки перед закрытием.
#[tauri::command]
pub fn editor_ack_flush(editor: State<'_, Arc<EditorState>>) {
    editor.acked.store(true, Ordering::SeqCst);
}

/// Правки сохранены (или отброшены пользователем): закрыть окно.
#[tauri::command]
pub fn editor_close_now(app: tauri::AppHandle, editor: State<'_, Arc<EditorState>>) {
    editor.dirty.store(false, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.close();
    }
}

/// Закрытие окна при несохранённых правках: окно остаётся, странице отправляется запрос сохранить.
/// Возвращает `true`, если закрытие нужно отложить.
pub fn defer_close_for_unsaved(
    window: &tauri::Window,
    editor: &Arc<EditorState>,
    subscribers: &Subscribers,
) -> bool {
    if !editor.is_dirty() {
        return false;
    }
    log::info!("Закрытие окна отложено: есть несохранённые правки, страница сохраняет их");
    editor.acked.store(false, Ordering::SeqCst);
    subscribers.publish(AppEvent::Request(EditorRequest::FlushAndClose));
    let (window, editor) = (window.clone(), Arc::clone(editor));
    std::thread::spawn(move || {
        std::thread::sleep(FLUSH_TIMEOUT);
        if editor.is_dirty() && !editor.acked.load(Ordering::SeqCst) {
            log::error!("Страница не ответила на запрос сохранения, окно закрывается");
            editor.dirty.store(false, Ordering::SeqCst);
            let _ = window.close();
        }
    });
    true
}
