//! Оболочка приложения dev-tolk (Tauri 2). На этапе MVP-012 — только каркас:
//! окно, один экземпляр, каталог данных, журнал. Бизнес-логика добавляется позже.

mod debug_transcribe;

use std::path::PathBuf;

use serde::Serialize;
use tauri::{Manager, Runtime};
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};
use tolk_core::layout::DataLayout;
use tolk_core::store::Store;

/// Размер файла журнала, после которого он ротируется, в байтах.
const LOG_MAX_FILE_BYTES: u128 = 2 * 1024 * 1024;
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
            app.manage(store);
            app.manage(debug_transcribe::ActiveRun::default());
            // Запрет сна на время обработки: исполнитель очереди (MVP-016) вызывает `sync_with_queue` после переходов.
            app.manage(std::sync::Mutex::new(tolk_core::power::SleepInhibitor::new()));

            log::info!(
                "dev-tolk {} запущен ({} {}), данные: {}",
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH,
                data.display()
            );
            Ok(())
        })
        .on_window_event(|window, event| {
            // Закрытие окна отменяет идущую обработку: дочерний процесс не остаётся (при аварийном
            // завершении приложения движок завершается сам, см. `--exit-on-stdin-close`).
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                window.state::<debug_transcribe::ActiveRun>().cancel();
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            report_csp_check,
            debug_transcribe::debug_models,
            debug_transcribe::debug_transcribe,
            debug_transcribe::debug_cancel
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить приложение");
}
