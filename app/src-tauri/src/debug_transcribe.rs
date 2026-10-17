//! Временная отладочная функция «транскрибировать файл» (MVP-013). Удаляется в MVP-019, когда появится
//! экран очереди: задания тогда запускает очередь (MVP-016), а не кнопка.
//!
//! Показывает, что цепочка работает из собранного приложения: сайдкар запускается, события приходят
//! в интерфейс через `Channel`, отмена завершает процесс, интерфейс не блокируется (запуск идёт в
//! отдельном потоке). Тексты сегментов уходят только в интерфейс, в журнал они не пишутся.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tolk_core::download::{is_installed, model_path, CancelToken};
use tolk_core::engine::{self, EngineEvent, RunResult, TranscribeArgs};
use tolk_core::layout::DataLayout;
use tolk_core::model_manager;
use tolk_core::models::{find, SystemInfo, VAD_MODEL_ID};
use tolk_core::store::Store;
use tolk_core::user_errors;

/// Запущенное в данный момент задание (одно: защита от двойного запуска).
#[derive(Default)]
pub struct ActiveRun(Mutex<Option<CancelToken>>);

impl ActiveRun {
    /// Отменяет идущее задание (например, при закрытии окна).
    pub fn cancel(&self) {
        if let Some(token) = self.0.lock().ok().and_then(|g| g.clone()) {
            token.cancel();
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugModel {
    id: String,
    title: String,
    installed: bool,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UiEvent {
    Progress {
        percent: u8,
    },
    Segment {
        start_ms: u64,
        end_ms: u64,
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        segments: usize,
        audio_ms: u64,
        elapsed_ms: u64,
        model_load_ms: u64,
        recognize_ms: u64,
        peak_rss_mb: Option<u64>,
        backend: String,
    },
    Failed {
        code: String,
        message: String,
        action: String,
    },
    Cancelled,
}

/// Путь к сайдкару: рядом с исполняемым файлом приложения (в собранном `.app` это `Contents/MacOS`),
/// в разработке — `TOLK_ENGINE` или `target/{release,debug}`.
fn engine_exe() -> Option<PathBuf> {
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
pub fn debug_models(app: AppHandle, store: State<'_, Store>) -> Result<Vec<DebugModel>, String> {
    let layout = DataLayout::new(app.path().app_local_data_dir().map_err(|e| e.to_string())?);
    let list = model_manager::list(&store.db, &layout.models_dir(), &SystemInfo::detect())
        .map_err(|e| user_errors::from_store(&e).message)?;
    Ok(list
        .into_iter()
        .map(|s| DebugModel {
            id: s.entry.id.clone(),
            title: s.entry.title.clone(),
            installed: s.installed,
        })
        .collect())
}

#[tauri::command]
pub fn debug_cancel(active: State<'_, ActiveRun>) {
    active.cancel();
}

/// Запускает распознавание файла выбранной моделью. Событиями управляет интерфейс через `Channel`.
#[tauri::command]
pub async fn debug_transcribe(
    app: AppHandle,
    active: State<'_, ActiveRun>,
    file: String,
    model_id: String,
    on_event: Channel<UiEvent>,
) -> Result<(), String> {
    let token = CancelToken::new();
    {
        let mut slot = active
            .0
            .lock()
            .map_err(|_| "внутренняя ошибка".to_string())?;
        if slot.is_some() {
            return Err("Обработка уже идёт: дождитесь окончания или отмените её.".into());
        }
        *slot = Some(token.clone());
    }
    let layout = DataLayout::new(app.path().app_local_data_dir().map_err(|e| e.to_string())?);
    let app_for_task = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run(&layout, &file, &model_id, &token, &on_event)
    })
    .await;
    if let Ok(mut slot) = app_for_task.state::<ActiveRun>().0.lock() {
        *slot = None;
    }
    match result {
        Ok(r) => r,
        Err(e) => {
            log::error!("Поток обработки завершился аварийно: {e}");
            Err("Внутренняя ошибка при обработке.".into())
        }
    }
}

fn run(
    layout: &DataLayout,
    file: &str,
    model_id: &str,
    cancel: &CancelToken,
    ui: &Channel<UiEvent>,
) -> Result<(), String> {
    let models_dir = layout.models_dir();
    let entry = find(model_id).ok_or("Такой модели нет в каталоге.")?;
    if !is_installed(entry, &models_dir) {
        return Err("Модель не установлена: установите её на экране «Модели».".into());
    }
    let vad = find(VAD_MODEL_ID)
        .filter(|v| is_installed(v, &models_dir))
        .ok_or("Не установлена модель определения речи.")?;
    let exe = engine_exe().ok_or("Не найден движок распознавания (tolk-engine).")?;

    let output = layout
        .tmp_dir()
        .join(format!("debug-{}.json", tolk_core::jobs::now_ms()));
    let args = TranscribeArgs {
        model: model_path(entry, &models_dir),
        vad_model: Some(model_path(vad, &models_dir)),
        input: PathBuf::from(file),
        output: output.clone(),
        language: "ru".into(),
        threads: None,
        no_gpu: false,
    };
    log::info!("Распознавание: модель {model_id}, движок {}", exe.display());
    let log_path = engine::engine_log_path(layout);
    let send = |event: UiEvent| {
        let _ = ui.send(event);
    };
    let result =
        engine::run_transcribe(
            &exe,
            &args,
            Some(&log_path),
            cancel,
            &mut |event| match event {
                EngineEvent::Progress { percent } => send(UiEvent::Progress { percent: *percent }),
                EngineEvent::Segment {
                    start_ms,
                    end_ms,
                    text,
                } => send(UiEvent::Segment {
                    start_ms: *start_ms,
                    end_ms: *end_ms,
                    text: text.clone(),
                }),
                EngineEvent::Done(_) | EngineEvent::Error(_) => {}
            },
        )
        .map_err(|e| {
            log::error!("Не удалось запустить движок: {e}");
            "Не удалось запустить движок распознавания.".to_string()
        })?;
    let _ = std::fs::remove_file(&output);

    match result {
        RunResult::Done(d) => {
            log::info!(
                "Распознавание завершено: {} сегментов, {} мс, бэкенд {}",
                d.segments,
                d.elapsed_ms,
                d.backend
            );
            send(UiEvent::Done {
                segments: d.segments,
                audio_ms: d.audio_ms,
                elapsed_ms: d.elapsed_ms,
                model_load_ms: d.model_load_ms,
                recognize_ms: d.recognize_ms,
                peak_rss_mb: d.peak_rss_bytes.map(|b| b / (1024 * 1024)),
                backend: d.backend,
            });
        }
        RunResult::Failed(e) => {
            log::warn!("Движок сообщил об ошибке: {}", e.code);
            let m = user_errors::from_engine(&e.code, e.codec.as_deref());
            send(UiEvent::Failed {
                code: m.code,
                message: m.message,
                action: m.action,
            });
        }
        RunResult::Cancelled => {
            log::info!("Распознавание отменено пользователем");
            send(UiEvent::Cancelled);
        }
        RunResult::Crashed {
            exit_code,
            stderr_tail,
        } => {
            log::error!(
                "Движок завершился аварийно (код {exit_code:?}); хвост stderr: {stderr_tail}"
            );
            let m = user_errors::from_engine("INTERNAL", None);
            send(UiEvent::Failed {
                code: m.code,
                message: m.message,
                action: m.action,
            });
        }
    }
    Ok(())
}
