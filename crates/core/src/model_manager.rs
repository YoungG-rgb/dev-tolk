//! Управление установленными моделями: список, модель по умолчанию, удаление, предупреждение о памяти
//! (TD-13, TD-14, MVP-024). Загрузка — в модуле `download`.
//!
//! Функции принимают `&Db` и сами берут соединение: вызывать их, держа `db.conn()`, нельзя (взаимная блокировка).

use std::fs;
use std::path::Path;

use crate::db::Db;
use crate::download::{is_installed, model_path, partial_bytes};
use crate::error::{Result, StoreError};
use crate::models::{catalog, find, ModelEntry, ModelKind, SystemInfo, VAD_MODEL_ID};

const SETTING_DEFAULT_MODEL: &str = "default_model";

/// Запас памяти сверх оценки потребности модели: предупреждение показывается, если доступно меньше
/// `оценка × MEMORY_HEADROOM`. Значение предварительное, уточняется измерениями (TD-14).
pub const MEMORY_HEADROOM: f64 = 1.5;

#[derive(Debug, Clone)]
pub struct ModelStatus {
    pub entry: &'static ModelEntry,
    pub installed: bool,
    /// Размер недокачанной части, байт (0 — загрузка не начиналась).
    pub partial_bytes: u64,
    pub is_default: bool,
    pub recommended: bool,
}

/// Модели распознавания каталога со статусами. Модель определения речи — служебный компонент
/// и в список выбора не входит.
pub fn list(db: &Db, models_dir: &Path, system: &SystemInfo) -> Result<Vec<ModelStatus>> {
    let default_id = default_model(db, models_dir)?.map(|m| m.id.clone());
    let recommended = crate::models::recommend(system).model_id;
    Ok(catalog()
        .iter()
        .filter(|m| m.kind == ModelKind::Asr)
        .map(|entry| ModelStatus {
            entry,
            installed: is_installed(entry, models_dir),
            partial_bytes: partial_bytes(entry, models_dir),
            is_default: default_id.as_deref() == Some(entry.id.as_str()),
            recommended: entry.id == recommended,
        })
        .collect())
}

/// Нужен ли мастер первого запуска: ни одной установленной модели распознавания.
pub fn needs_first_run(models_dir: &Path) -> bool {
    !catalog()
        .iter()
        .any(|m| m.kind == ModelKind::Asr && is_installed(m, models_dir))
}

/// Модель по умолчанию. `None`, если не выбрана или выбранная модель больше не установлена.
pub fn default_model(db: &Db, models_dir: &Path) -> Result<Option<&'static ModelEntry>> {
    Ok(db
        .setting(SETTING_DEFAULT_MODEL)?
        .and_then(|id| find(&id))
        .filter(|m| m.kind == ModelKind::Asr && is_installed(m, models_dir)))
}

/// Выбор модели по умолчанию. Применяется к заданиям, созданным после этого; уже созданные
/// сохраняют модель, указанную при создании.
pub fn set_default_model(db: &Db, models_dir: &Path, id: &str) -> Result<()> {
    let entry = find(id)
        .filter(|m| m.kind == ModelKind::Asr)
        .ok_or_else(|| StoreError::UnknownModel(id.to_string()))?;
    if !is_installed(entry, models_dir) {
        return Err(StoreError::ModelNotInstalled(id.to_string()));
    }
    db.set_setting(SETTING_DEFAULT_MODEL, id)
}

/// Идентификатор модели для нового задания: модель по умолчанию. Первая установленная модель
/// становится моделью по умолчанию, если выбор ещё не делался.
pub fn model_for_new_job(db: &Db, models_dir: &Path) -> Result<Option<String>> {
    if let Some(m) = default_model(db, models_dir)? {
        return Ok(Some(m.id.clone()));
    }
    let first = catalog()
        .iter()
        .find(|m| m.kind == ModelKind::Asr && is_installed(m, models_dir));
    if let Some(m) = first {
        db.set_setting(SETTING_DEFAULT_MODEL, &m.id)?;
        return Ok(Some(m.id.clone()));
    }
    Ok(None)
}

/// Удаляет файл модели (и недокачанную часть). Запрещено, пока модель используется заданием в обработке.
/// Если удалена модель по умолчанию, выбор сбрасывается.
pub fn delete_model(db: &Db, models_dir: &Path, id: &str) -> Result<()> {
    let entry = find(id).ok_or_else(|| StoreError::UnknownModel(id.to_string()))?;
    if entry.id == VAD_MODEL_ID {
        return Err(StoreError::UnknownModel(id.to_string())); // служебный компонент удалению пользователем не подлежит
    }
    let in_use: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM jobs WHERE status = 'PROCESSING' AND model_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    if in_use > 0 {
        return Err(StoreError::ModelInUse(id.to_string()));
    }
    for path in [
        model_path(entry, models_dir),
        models_dir.join(format!("{}.part", entry.file)),
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if db.setting(SETTING_DEFAULT_MODEL)?.as_deref() == Some(id) {
        db.conn().execute(
            "DELETE FROM settings WHERE key = ?1",
            [SETTING_DEFAULT_MODEL],
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWarning {
    pub required_mb: u64,
    pub available_mb: u64,
}

/// Предупреждение перед запуском задания, если свободной памяти меньше потребности модели с запасом.
/// Запуск не блокируется. Для моделей без оценки потребности предупреждения нет: оценка появится
/// после измерений (MVP-007, MVP-038).
pub fn memory_warning(entry: &ModelEntry, system: &SystemInfo) -> Option<MemoryWarning> {
    let estimate = u64::from(entry.ram_estimate_mb?);
    let required_mb = (estimate as f64 * MEMORY_HEADROOM).ceil() as u64;
    let available_mb = system.available_ram_bytes / (1024 * 1024);
    (available_mb < required_mb).then_some(MemoryWarning {
        required_mb,
        available_mb,
    })
}
