//! Экран «Модели» и первый запуск (MVP-024): список, загрузка по явному согласию, импорт из файла,
//! модель по умолчанию, удаление. Сеть используется только внутри `model_download`, то есть после нажатия
//! пользователем кнопки «Скачать»; сам по себе запуск приложения ничего не скачивает.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tolk_core::download::{self, CancelToken, DownloadConfig, DownloadError};
use tolk_core::model_manager::{self, memory_warning};
use tolk_core::models::{find, recommend, ModelKind, SystemInfo};
use tolk_core::store::Store;
use tolk_core::user_errors;

use crate::queue_commands::{AppEvent, Subscribers};

/// Идущая операция с моделью (загрузка или импорт) и её токен отмены. Одна за раз.
#[derive(Default)]
pub struct ModelTask(Mutex<Option<CancelToken>>);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ModelEvent {
    #[serde(rename_all = "camelCase")]
    DownloadProgress {
        title: String,
        file_index: usize,
        file_count: usize,
        downloaded: u64,
        total: u64,
        bytes_per_sec: f64,
    },
    #[serde(rename_all = "camelCase")]
    ModelTaskDone {
        ok: bool,
        cancelled: bool,
        message: Option<String>,
        action: Option<String>,
        suggest_import: bool,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDto {
    id: String,
    title: String,
    size_bytes: u64,
    /// Сколько нужно скачать, чтобы модель заработала (с моделью определения речи, если её нет).
    required_bytes: u64,
    partial_bytes: u64,
    installed: bool,
    is_default: bool,
    recommended: bool,
    ram_estimate_mb: Option<u32>,
    source: String,
    license: String,
    note: Option<String>,
    memory_warning: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewDto {
    models: Vec<ModelDto>,
    first_run: bool,
    os: String,
    total_ram_mb: u64,
    available_ram_mb: u64,
    recommendation: String,
    low_memory_warning: bool,
    busy: bool,
}

fn user_error(e: &tolk_core::StoreError) -> String {
    let m = user_errors::from_store(e);
    log::warn!("Команда моделей не выполнена: {:?}", m.technical);
    m.message
}

pub fn memory_text(entry: &tolk_core::models::ModelEntry, system: &SystemInfo) -> Option<String> {
    memory_warning(entry, system).map(|w| {
        format!(
            "Свободной памяти может не хватить: модели нужно около {} МБ, сейчас доступно около {} МБ. Закройте другие программы; запуск не блокируется.",
            w.required_mb, w.available_mb
        )
    })
}

#[tauri::command]
pub fn models_overview(
    store: State<'_, Arc<Store>>,
    task: State<'_, Arc<ModelTask>>,
) -> Result<OverviewDto, String> {
    let models_dir = store.files.layout().models_dir();
    let system = SystemInfo::detect();
    let list = model_manager::list(&store.db, &models_dir, &system).map_err(|e| user_error(&e))?;
    let rec = recommend(&system);
    let rec_entry = find(rec.model_id);
    let recommendation = match rec_entry {
        Some(e) => format!(
            "Рекомендуем «{}» для компьютера с {} ГБ памяти. Рекомендация предварительная: пороги ещё не подтверждены измерениями на ваших компьютерах.",
            e.title,
            (system.total_ram_bytes as f64 / (1024.0 * 1024.0 * 1024.0)).round()
        ),
        None => String::new(),
    };
    Ok(OverviewDto {
        first_run: model_manager::needs_first_run(&models_dir),
        os: system.os.to_string(),
        total_ram_mb: system.total_ram_bytes / (1024 * 1024),
        available_ram_mb: system.available_ram_bytes / (1024 * 1024),
        recommendation,
        low_memory_warning: rec.low_memory_warning,
        busy: task.0.lock().unwrap_or_else(|e| e.into_inner()).is_some(),
        models: list
            .into_iter()
            .map(|s| ModelDto {
                id: s.entry.id.clone(),
                title: s.entry.title.clone(),
                size_bytes: s.entry.size,
                required_bytes: download::required_downloads(&s.entry.id, &models_dir)
                    .map(|v| {
                        v.iter()
                            .map(|e| e.size)
                            .sum::<u64>()
                            .saturating_sub(s.partial_bytes)
                    })
                    .unwrap_or(0),
                partial_bytes: s.partial_bytes,
                installed: s.installed,
                is_default: s.is_default,
                recommended: s.recommended,
                ram_estimate_mb: s.entry.ram_estimate_mb,
                source: s.entry.source.clone(),
                license: s.entry.license.clone(),
                note: s.entry.note.clone(),
                memory_warning: memory_text(s.entry, &system),
            })
            .collect(),
    })
}

#[tauri::command]
pub fn model_set_default(store: State<'_, Arc<Store>>, id: String) -> Result<(), String> {
    let models_dir = store.files.layout().models_dir();
    model_manager::set_default_model(&store.db, &models_dir, &id).map_err(|e| user_error(&e))
}

#[tauri::command]
pub fn model_delete(store: State<'_, Arc<Store>>, id: String) -> Result<(), String> {
    let models_dir = store.files.layout().models_dir();
    model_manager::delete_model(&store.db, &models_dir, &id).map_err(|e| user_error(&e))
}

fn begin(task: &ModelTask) -> Option<CancelToken> {
    let mut slot = task.0.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() {
        return None;
    }
    let token = CancelToken::new();
    *slot = Some(token.clone());
    Some(token)
}

fn done_event(
    ok: bool,
    cancelled: bool,
    message: Option<String>,
    action: Option<String>,
    suggest_import: bool,
) -> AppEvent {
    AppEvent::Model(ModelEvent::ModelTaskDone {
        ok,
        cancelled,
        message,
        action,
        suggest_import,
    })
}

/// Загрузка модели и, если её нет, модели определения речи. Вызывается только кнопкой «Скачать»:
/// это и есть согласие пользователя на обращение к сети.
#[tauri::command]
pub fn model_download(
    store: State<'_, Arc<Store>>,
    task: State<'_, Arc<ModelTask>>,
    subscribers: State<'_, Arc<Subscribers>>,
    id: String,
) -> Result<(), String> {
    let entry = find(&id)
        .filter(|m| m.kind == ModelKind::Asr)
        .ok_or("Такой модели нет в каталоге.")?;
    let models_dir = store.files.layout().models_dir();
    let required = download::required_downloads(&entry.id, &models_dir).unwrap_or_default();
    let token = begin(&task)
        .ok_or("Другая операция с моделью ещё идёт: дождитесь окончания или отмените её.")?;
    let (store, task, subscribers) = (
        Arc::clone(&store),
        Arc::clone(&task),
        Arc::clone(&subscribers),
    );
    let model_id = entry.id.clone();
    std::thread::spawn(move || {
        let cfg = DownloadConfig::production();
        let count = required.len();
        let mut outcome: Result<(), DownloadError> = Ok(());
        for (index, item) in required.iter().enumerate() {
            let mut last = Instant::now() - Duration::from_secs(1);
            let sink = Arc::clone(&subscribers);
            let title = item.title.clone();
            let result = download::download(item, &models_dir, &cfg, &token, &mut |p| {
                if last.elapsed() >= Duration::from_millis(250) || p.downloaded >= p.total {
                    last = Instant::now();
                    sink.publish(AppEvent::Model(ModelEvent::DownloadProgress {
                        title: title.clone(),
                        file_index: index,
                        file_count: count,
                        downloaded: p.downloaded,
                        total: p.total,
                        bytes_per_sec: p.bytes_per_sec,
                    }));
                }
            });
            if let Err(e) = result {
                outcome = Err(e);
                break;
            }
        }
        let event = match outcome {
            Ok(()) => {
                // Первая установленная модель становится моделью по умолчанию.
                let _ = model_manager::model_for_new_job(&store.db, &models_dir);
                log::info!("Модель {model_id} установлена");
                done_event(true, false, None, None, false)
            }
            Err(DownloadError::Cancelled) => done_event(false, true, None, None, false),
            Err(e) => {
                let m = user_errors::from_download(&e);
                log::warn!("Загрузка модели не удалась: {} {:?}", m.code, m.technical);
                done_event(
                    false,
                    false,
                    Some(m.message),
                    Some(m.action),
                    e.suggest_import(),
                )
            }
        };
        *task.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
        subscribers.publish(event);
    });
    Ok(())
}

#[tauri::command]
pub fn model_task_cancel(task: State<'_, Arc<ModelTask>>) {
    if let Some(token) = task.0.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        token.cancel();
    }
}

/// Импорт модели из файла (флешка, файл от коллеги): без сети, файл проверяется по размеру и SHA-256.
#[tauri::command]
pub async fn model_import_pick(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
    task: State<'_, Arc<ModelTask>>,
    subscribers: State<'_, Arc<Subscribers>>,
) -> Result<(), String> {
    let picked = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            app.dialog()
                .file()
                .set_title("Выберите файл модели")
                .add_filter("Модель (*.bin)", &["bin"])
                .blocking_pick_file()
        })
        .await
        .map_err(|_| "Не удалось открыть окно выбора файла.".to_string())?
    };
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(());
    };
    start_import(&store, &task, &subscribers, path)
}

fn start_import(
    store: &Arc<Store>,
    task: &Arc<ModelTask>,
    subscribers: &Arc<Subscribers>,
    path: PathBuf,
) -> Result<(), String> {
    let token = begin(task)
        .ok_or("Другая операция с моделью ещё идёт: дождитесь окончания или отмените её.")?;
    let (store, task, subscribers) = (Arc::clone(store), Arc::clone(task), Arc::clone(subscribers));
    std::thread::spawn(move || {
        let models_dir = store.files.layout().models_dir();
        let event = match download::import_from_file(&path, &models_dir, &token) {
            Ok(entry) => {
                // Модель определения речи нужна для работы: если её нет, это видно на экране, а предложение
                // скачать её (около 1 МБ) — кнопка «Скачать» у основной модели.
                let _ = model_manager::model_for_new_job(&store.db, &models_dir);
                log::info!("Модель {} импортирована из файла", entry.id);
                done_event(true, false, None, None, false)
            }
            Err(DownloadError::Cancelled) => done_event(false, true, None, None, false),
            Err(e) => {
                let m = user_errors::from_download(&e);
                done_event(false, false, Some(m.message), Some(m.action), false)
            }
        };
        *task.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
        subscribers.publish(event);
    });
    Ok(())
}

/// Импорт модели по пути (только автопроверка в окне, feature `ui-test`).
#[cfg(feature = "ui-test")]
#[tauri::command]
pub fn model_import_path(
    store: State<'_, Arc<Store>>,
    task: State<'_, Arc<ModelTask>>,
    subscribers: State<'_, Arc<Subscribers>>,
    path: String,
) -> Result<(), String> {
    start_import(&store, &task, &subscribers, PathBuf::from(path))
}
