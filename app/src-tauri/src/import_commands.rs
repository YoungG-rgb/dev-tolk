//! Импорт файлов: выбор в диалоге, перетаскивание в окно, ввод пути (временная отладка). Все три способа
//! приходят в одну функцию `start_import`; ход импорта и итоги идут в интерфейс событиями `ImportEvent`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tolk_core::download::CancelToken;
use tolk_core::executor::Executor;
use tolk_core::import::{self, ImportEvent, AUDIO_EXTENSIONS, VIDEO_EXTENSIONS};
use tolk_core::model_manager;
use tolk_core::store::Store;

use crate::queue_commands::{engine_exe, AppEvent, Subscribers};

/// Идущий импорт (один пакет за раз) и его токен отмены.
#[derive(Default)]
pub struct ImportState(Mutex<Option<CancelToken>>);

impl ImportState {
    pub fn cancel(&self) {
        if let Some(token) = self.0.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            token.cancel();
        }
    }
}

/// Запускает импорт пакета файлов в отдельном потоке. Если предыдущий пакет ещё идёт, сообщает `ImportBusy`.
pub fn start_import(app: &AppHandle, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let state = app.state::<ImportState>();
    let subscribers = Arc::clone(&app.state::<Arc<Subscribers>>());
    let token = CancelToken::new();
    {
        let mut slot = state.0.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            subscribers.publish(AppEvent::Import(ImportEvent::ImportBusy));
            return;
        }
        *slot = Some(token.clone());
    }
    let store = Arc::clone(&app.state::<Arc<Store>>());
    let executor = Arc::clone(&app.state::<Arc<Executor>>());
    let app = app.clone();
    std::thread::spawn(move || {
        let exe = engine_exe().unwrap_or_else(|| PathBuf::from("tolk-engine"));
        let models_dir = store.files.layout().models_dir();
        let model_id = model_manager::model_for_new_job(&store.db, &models_dir)
            .ok()
            .flatten();
        let sink = Arc::clone(&subscribers);
        // Предупреждения перед обработкой: запуск при этом не блокируется.
        match model_id.as_deref().and_then(tolk_core::models::find) {
            None => sink.publish(AppEvent::Import(ImportEvent::ImportNotice {
                message: "Модель распознавания не установлена. Записи будут добавлены, а обработка начнётся после установки модели на вкладке «Модели» (затем нажмите «Повторить»).".into(),
            })),
            Some(entry) => {
                if let Some(message) = crate::models_commands::memory_text(entry, &tolk_core::models::SystemInfo::detect()) {
                    sink.publish(AppEvent::Import(ImportEvent::ImportNotice { message }));
                }
            }
        }
        import::import_many(
            &store,
            &exe,
            &paths,
            model_id.as_deref(),
            &token,
            &mut |event| sink.publish(AppEvent::Import(event)),
            &mut |_job| executor.notify(),
        );
        *app.state::<ImportState>()
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    });
}

/// Диалог выбора файлов (несколько файлов, аудио и видео MP4/MOV).
#[tauri::command]
pub async fn import_pick(app: AppHandle) -> Result<(), String> {
    let extensions: Vec<&str> = AUDIO_EXTENSIONS
        .iter()
        .chain(VIDEO_EXTENSIONS)
        .copied()
        .collect();
    let picked = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            app.dialog()
                .file()
                .set_title("Выберите записи")
                .add_filter("Аудио и видео", &extensions)
                .blocking_pick_files()
        })
        .await
        .map_err(|_| "Не удалось открыть окно выбора файлов.".to_string())?
    };
    let paths: Vec<PathBuf> = picked
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect();
    start_import(&app, paths);
    Ok(())
}

/// Импорт по путям (только автопроверка в окне, feature `ui-test`).
#[cfg(feature = "ui-test")]
#[tauri::command]
pub fn import_paths(app: AppHandle, paths: Vec<String>) {
    start_import(&app, paths.into_iter().map(PathBuf::from).collect());
}

#[tauri::command]
pub fn import_cancel(state: State<'_, ImportState>) {
    state.cancel();
}
