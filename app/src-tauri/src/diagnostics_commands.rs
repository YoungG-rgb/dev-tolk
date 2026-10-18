//! Вкладка «Диагностика» (MVP-025, MVP-038): папка журналов, отчёт о производительности, оценка качества.
//! Приложение ничего не отправляет: отчёт сохраняется файлом, который пользователь передаёт сам.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, State};
use tolk_core::jobs::now_ms;
use tolk_core::metrics;
use tolk_core::models::SystemInfo;
use tolk_core::store::Store;
use tolk_core::user_errors;

fn user_error(e: &tolk_core::StoreError) -> String {
    let m = user_errors::from_store(e);
    log::warn!("Команда диагностики не выполнена: {:?}", m.technical);
    m.message
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsDto {
    version: String,
    os: String,
    arch: String,
    data_dir: String,
    log_dir: String,
    /// Занято записями (аудио и расшифровки), байт.
    jobs_bytes: u64,
    models_bytes: u64,
    metrics_count: usize,
}

fn dir_size(path: &std::path::Path) -> u64 {
    std::fs::read_dir(path)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_size(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

#[tauri::command]
pub fn diagnostics_info(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
) -> Result<DiagnosticsDto, String> {
    let layout = store.files.layout();
    let metrics_count: i64 = store
        .db
        .conn()
        .query_row("SELECT COUNT(*) FROM job_metrics", [], |r| r.get(0))
        .map_err(|e| user_error(&e.into()))?;
    Ok(DiagnosticsDto {
        version: app.package_info().version.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        data_dir: layout.root().to_string_lossy().into_owned(),
        log_dir: layout.logs_dir().to_string_lossy().into_owned(),
        jobs_bytes: store.files.total_size(),
        models_bytes: dir_size(&layout.models_dir()),
        metrics_count: metrics_count as usize,
    })
}

/// Открывает папку в файловом менеджере. В автопроверке (`TOLK_UI_TEST_DATA`) окна не открываются.
fn open_dir(dir: PathBuf) -> Result<(), String> {
    #[cfg(feature = "ui-test")]
    if std::env::var_os("TOLK_UI_TEST_DATA").is_some() {
        log::info!("ui-test: открытие папки пропущено");
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| user_error(&e.into()))?;
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            log::warn!("Не удалось открыть папку: {e}");
            "Не удалось открыть папку. Откройте её вручную: путь показан на экране.".to_string()
        })
}

#[tauri::command]
pub fn open_logs_dir(store: State<'_, Arc<Store>>) -> Result<(), String> {
    open_dir(store.files.layout().logs_dir())
}

/// Сохраняет отчёт о производительности (система и метрики заданий, без текстов и имён файлов).
/// `Ok(None)` — пользователь закрыл диалог.
#[tauri::command]
pub async fn report_save(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let date = tolk_core::glossary::iso8601_utc(now_ms());
    let name = format!("dev-tolk-отчёт-{}.json", &date[..10]);
    #[cfg(feature = "ui-test")]
    let forced =
        std::env::var_os("TOLK_UI_TEST_EXPORT_DIR").map(|dir| PathBuf::from(dir).join(&name));
    #[cfg(not(feature = "ui-test"))]
    let forced: Option<PathBuf> = None;

    let dest = match forced {
        Some(p) => p,
        None => {
            let (dialog_app, dialog_name) = (app.clone(), name.clone());
            let picked = tauri::async_runtime::spawn_blocking(move || {
                dialog_app
                    .dialog()
                    .file()
                    .set_title("Сохранить отчёт")
                    .set_file_name(dialog_name)
                    .add_filter("Отчёт (*.json)", &["json"])
                    .blocking_save_file()
            })
            .await
            .map_err(|_| "Не удалось открыть окно сохранения.".to_string())?;
            let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
                return Ok(None);
            };
            path
        }
    };
    let version = app.package_info().version.to_string();
    let store = Arc::clone(&store);
    let written = dest.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = store.db.conn();
        metrics::save_report(&conn, &SystemInfo::detect(), &version, &dest)
    })
    .await
    .map_err(|_| "Внутренняя ошибка при сохранении отчёта.".to_string())?
    .map_err(|e| user_error(&e))?;
    Ok(Some(
        written
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(name),
    ))
}

/// Оценка качества расшифровки пользователем, 1–5 (попадает в отчёт о производительности).
#[tauri::command]
pub fn job_rate(store: State<'_, Arc<Store>>, id: String, rating: u8) -> Result<(), String> {
    let conn = store.db.conn();
    metrics::set_rating(&conn, &id, rating).map_err(|e| user_error(&e))
}
