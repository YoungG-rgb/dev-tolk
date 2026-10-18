//! Оболочка приложения dev-tolk (Tauri 2). На этапе MVP-012 — только каркас:
//! окно, один экземпляр, каталог данных, журнал. Бизнес-логика добавляется позже.

mod diagnostics_commands;
mod glossary_commands;
mod import_commands;
mod models_commands;
mod queue_commands;
#[cfg(feature = "ui-test")]
mod ui_test;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{Manager, Runtime};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};
use tolk_core::executor::Executor;
use tolk_core::layout::DataLayout;
use tolk_core::store::Store;

/// Размер файла журнала, после которого он ротируется, в байтах.
const LOG_MAX_FILE_BYTES: u128 = 2 * 1024 * 1024;
/// Сколько ждать записи итога идущего задания при закрытии окна.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);
/// Сколько файлов журнала хранится.
const LOG_KEEP_FILES: usize = 5;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: String,
    platform: &'static str,
    arch: &'static str,
    data_dir: String,
    log_dir: String,
}

fn data_dir<R: Runtime, M: Manager<R>>(manager: &M) -> Result<PathBuf, String> {
    // Автопроверка (feature ui-test) работает в отдельном каталоге данных, не трогая данные пользователя.
    #[cfg(feature = "ui-test")]
    if let Some(dir) = std::env::var_os("TOLK_UI_TEST_DATA") {
        return Ok(PathBuf::from(dir));
    }
    manager
        .path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn app_info(app: tauri::AppHandle) -> Result<AppInfo, String> {
    let data = data_dir(&app)?;
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        platform: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        log_dir: data.join("logs").to_string_lossy().into_owned(),
        data_dir: data.to_string_lossy().into_owned(),
    })
}

/// Результат самопроверки CSP на странице: попадает в журнал, чтобы его можно было
/// проверить в собранном приложении без инструментов разработчика.
#[tauri::command]
fn report_csp_check(blocked: bool) {
    if blocked {
        log::info!("Самопроверка CSP: внешний запрос заблокирован политикой");
    } else {
        log::error!("Самопроверка CSP: внешний запрос НЕ заблокирован");
    }
}

#[cfg(not(feature = "ui-test"))]
macro_rules! handlers {
    () => {
        tauri::generate_handler![
            app_info,
            report_csp_check,
            queue_commands::jobs_list,
            queue_commands::jobs_subscribe,
            queue_commands::job_cancel,
            queue_commands::job_retry,
            queue_commands::job_delete,
            queue_commands::transcript_load,
            queue_commands::transcript_save,
            queue_commands::transcript_revert,
            queue_commands::transcript_export,
            queue_commands::editor_set_dirty,
            queue_commands::editor_ack_flush,
            queue_commands::editor_close_now,
            glossary_commands::glossary_list,
            glossary_commands::glossary_add,
            glossary_commands::glossary_update,
            glossary_commands::glossary_set_active,
            glossary_commands::glossary_delete,
            glossary_commands::glossary_export,
            glossary_commands::glossary_import_pick,
            diagnostics_commands::diagnostics_info,
            diagnostics_commands::open_logs_dir,
            diagnostics_commands::report_save,
            diagnostics_commands::job_rate,
            models_commands::models_overview,
            models_commands::model_set_default,
            models_commands::model_delete,
            models_commands::model_download,
            models_commands::model_task_cancel,
            models_commands::model_import_pick,
            import_commands::import_pick,
            import_commands::import_cancel
        ]
    };
}

#[cfg(feature = "ui-test")]
macro_rules! handlers {
    () => {
        tauri::generate_handler![
            app_info,
            report_csp_check,
            queue_commands::jobs_list,
            queue_commands::jobs_subscribe,
            queue_commands::job_cancel,
            queue_commands::job_retry,
            queue_commands::job_delete,
            queue_commands::transcript_load,
            queue_commands::transcript_save,
            queue_commands::transcript_revert,
            queue_commands::transcript_export,
            queue_commands::editor_set_dirty,
            queue_commands::editor_ack_flush,
            queue_commands::editor_close_now,
            glossary_commands::glossary_list,
            glossary_commands::glossary_add,
            glossary_commands::glossary_update,
            glossary_commands::glossary_set_active,
            glossary_commands::glossary_delete,
            glossary_commands::glossary_export,
            glossary_commands::glossary_import_pick,
            diagnostics_commands::diagnostics_info,
            diagnostics_commands::open_logs_dir,
            diagnostics_commands::report_save,
            diagnostics_commands::job_rate,
            models_commands::models_overview,
            models_commands::model_set_default,
            models_commands::model_delete,
            models_commands::model_download,
            models_commands::model_task_cancel,
            models_commands::model_import_pick,
            import_commands::import_pick,
            import_commands::import_cancel,
            import_commands::import_paths,
            models_commands::model_import_path,
            glossary_commands::glossary_import_path,
            ui_test::ui_test_set_default_model,
            ui_test::ui_test_report,
            ui_test::ui_test_close
        ]
    };
}

pub fn run() {
    tauri::Builder::default()
        // Должен быть первым: второй запуск передаёт управление первому экземпляру и завершается.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data = data_dir(app.handle())?;
            let layout = DataLayout::new(&data);
            layout.ensure()?; // каталог `logs` нужен журналу до открытия хранилища

            let mut log = tauri_plugin_log::Builder::new()
                .clear_targets()
                .target(Target::new(TargetKind::Folder {
                    path: data.join("logs"),
                    file_name: Some("tolk".into()),
                }))
                .max_file_size(LOG_MAX_FILE_BYTES)
                .rotation_strategy(RotationStrategy::KeepSome(LOG_KEEP_FILES))
                .level(log::LevelFilter::Info);
            if cfg!(debug_assertions) {
                log = log.target(Target::new(TargetKind::Stderr));
            }
            app.handle().plugin(log.build())?;
            tolk_core::diagnostics::install_panic_hook(); // паники попадают в журнал

            let (store, cleanup) = Store::open(layout)?;
            log::info!(
                "Хранилище открыто, версия схемы {}; очистка при старте: прерванных заданий {}, временных файлов {}, каталогов без записи {}",
                store.db.schema_version()?,
                cleanup.interrupted,
                cleanup.tmp_removed,
                cleanup.orphans_removed.len()
            );
            let store = Arc::new(store);
            // Запрет сна на время обработки: исполнитель очереди вызывает `sync_with_queue` после переходов.
            let inhibitor = Arc::new(Mutex::new(tolk_core::power::SleepInhibitor::new()));
            let subscribers = Arc::new(queue_commands::Subscribers::default());
            let engine = queue_commands::engine_exe().unwrap_or_else(|| {
                log::error!("Не найден движок распознавания (tolk-engine)");
                PathBuf::from("tolk-engine")
            });
            let sink_subscribers = Arc::clone(&subscribers);
            let executor = Executor::new(
                Arc::clone(&store),
                engine,
                inhibitor,
                Arc::new(move |event| sink_subscribers.publish(queue_commands::AppEvent::Job(event))),
            );
            executor.spawn();
            app.manage(store);
            app.manage(subscribers);
            app.manage(Arc::new(queue_commands::EditorState::default()));
            app.manage(import_commands::ImportState::default());
            app.manage(Arc::new(models_commands::ModelTask::default()));
            app.manage(executor);

            log::info!(
                "dev-tolk {} запущен ({} {}), данные: {}",
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH,
                data.display()
            );
            #[cfg(feature = "ui-test")]
            ui_test::start(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Закрытие окна прерывает идущую обработку (задание получает статус «Обработка прервана»,
            // как при сбое); при аварийном завершении приложения движок завершается сам
            // (`--exit-on-stdin-close`).
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                import_commands::start_import(window.app_handle(), paths.clone());
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Несохранённые правки: окно не закрывается, страница сначала сохраняет их.
                let editor = window.state::<Arc<queue_commands::EditorState>>();
                if queue_commands::defer_close_for_unsaved(window, &editor, &window.state::<Arc<queue_commands::Subscribers>>()) {
                    api.prevent_close();
                    return;
                }
                window.state::<import_commands::ImportState>().cancel();
                let executor = window.state::<Arc<Executor>>();
                executor.shutdown();
                if !executor.wait_idle(SHUTDOWN_WAIT) {
                    log::warn!("Закрытие: задание не успело записать итог за {SHUTDOWN_WAIT:?}");
                }
            }
        })
        .invoke_handler(handlers!())
        .run(tauri::generate_context!())
        .expect("не удалось запустить приложение");
}
