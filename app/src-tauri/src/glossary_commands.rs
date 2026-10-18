//! Вкладка «Словарь» (MVP-036): термины пользователя, экспорт и импорт файла `*.tolk-glossary.json`.
//! Словарь хранится только локально; файлом им обмениваются вручную.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tolk_core::glossary::{self, ImportSummary, Term, TermInput};
use tolk_core::store::Store;
use tolk_core::user_errors;

fn user_error(e: &tolk_core::StoreError) -> String {
    let m = user_errors::from_store(e);
    log::warn!("Команда словаря не выполнена: {:?}", m.technical);
    m.message
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TermDto {
    id: i64,
    term: String,
    variants: Vec<String>,
    note: String,
    active: bool,
}

impl From<Term> for TermDto {
    fn from(t: Term) -> Self {
        Self {
            id: t.id,
            term: t.term,
            variants: t.variants,
            note: t.note,
            active: t.active,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryDto {
    added: usize,
    merged: usize,
    skipped: usize,
}

impl From<ImportSummary> for SummaryDto {
    fn from(s: ImportSummary) -> Self {
        Self {
            added: s.added,
            merged: s.merged,
            skipped: s.skipped,
        }
    }
}

#[tauri::command]
pub fn glossary_list(
    store: State<'_, Arc<Store>>,
    query: Option<String>,
) -> Result<Vec<TermDto>, String> {
    let conn = store.db.conn();
    Ok(
        glossary::list(&conn, query.as_deref().filter(|q| !q.trim().is_empty()))
            .map_err(|e| user_error(&e))?
            .into_iter()
            .map(Into::into)
            .collect(),
    )
}

#[tauri::command]
pub fn glossary_add(store: State<'_, Arc<Store>>, input: TermInput) -> Result<TermDto, String> {
    glossary::add(&store.db.conn(), &input)
        .map(Into::into)
        .map_err(|e| user_error(&e))
}

#[tauri::command]
pub fn glossary_update(
    store: State<'_, Arc<Store>>,
    id: i64,
    input: TermInput,
) -> Result<TermDto, String> {
    glossary::update(&store.db.conn(), id, &input)
        .map(Into::into)
        .map_err(|e| user_error(&e))
}

#[tauri::command]
pub fn glossary_set_active(
    store: State<'_, Arc<Store>>,
    id: i64,
    active: bool,
) -> Result<(), String> {
    glossary::set_active(&store.db.conn(), id, active).map_err(|e| user_error(&e))
}

#[tauri::command]
pub fn glossary_delete(store: State<'_, Arc<Store>>, id: i64) -> Result<(), String> {
    glossary::delete(&store.db.conn(), id)
        .map(|_| ())
        .map_err(|e| user_error(&e))
}

/// Экспорт словаря в файл. `Ok(None)` — диалог закрыт без сохранения.
#[tauri::command]
pub async fn glossary_export(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
) -> Result<Option<String>, String> {
    let name = format!("словарь{}", glossary::FILE_SUFFIX);
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
                    .set_title("Сохранить словарь")
                    .set_file_name(dialog_name)
                    .add_filter("Словарь dev-tolk", &["json"])
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
    let bytes = glossary::export_json(&store.db.conn()).map_err(|e| user_error(&e))?;
    tolk_core::export::write_file_atomic(&bytes, &dest).map_err(|e| user_error(&e))?;
    Ok(Some(
        dest.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(name),
    ))
}

fn import_file(store: &Store, path: &std::path::Path) -> Result<SummaryDto, String> {
    // Размер проверяется до чтения: чужой огромный файл целиком в память не читается.
    let len = std::fs::metadata(path)
        .map_err(|e| user_error(&e.into()))?
        .len();
    if len > glossary::MAX_FILE_BYTES as u64 {
        return Err("Файл слишком большой для словаря (больше 1 МБ).".into());
    }
    let bytes = std::fs::read(path).map_err(|e| user_error(&e.into()))?;
    glossary::import_json(&mut store.db.conn(), &bytes)
        .map(Into::into)
        .map_err(|e| user_error(&e))
}

/// Импорт словаря из файла со слиянием. `Ok(None)` — диалог закрыт.
#[tauri::command]
pub async fn glossary_import_pick(
    app: AppHandle,
    store: State<'_, Arc<Store>>,
) -> Result<Option<SummaryDto>, String> {
    let picked = {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            app.dialog()
                .file()
                .set_title("Выберите файл словаря")
                .add_filter("Словарь dev-tolk", &["json"])
                .blocking_pick_file()
        })
        .await
        .map_err(|_| "Не удалось открыть окно выбора файла.".to_string())?
    };
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    import_file(&store, &path).map(Some)
}

/// Импорт по пути (только автопроверка в окне, feature `ui-test`).
#[cfg(feature = "ui-test")]
#[tauri::command]
pub fn glossary_import_path(
    store: State<'_, Arc<Store>>,
    path: String,
) -> Result<SummaryDto, String> {
    import_file(&store, std::path::Path::new(&path))
}
