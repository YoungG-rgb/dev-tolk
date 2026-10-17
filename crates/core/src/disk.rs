//! Свободное место на диске (загрузка моделей, импорт файлов, экспорт).

use std::path::Path;

/// Сколько байт доступно пользователю на томе, где лежит `path` (каталог должен существовать).
pub fn available_bytes(path: &Path) -> std::io::Result<u64> {
    fs4::available_space(path)
}

/// Запас свободного места сверх необходимого.
pub const SPACE_MARGIN: u64 = 16 * 1024 * 1024;

/// Проверяет, что на томе, где лежит `path`, хватит места для `needed` байт (плюс запас).
/// Если узнать свободное место не удалось, проверка пропускается: ошибка записи всё равно будет понятной.
pub fn ensure_space(path: &Path, needed: u64) -> Result<(), crate::error::StoreError> {
    match available_bytes(path) {
        Ok(available) if available < needed.saturating_add(SPACE_MARGIN) => {
            Err(crate::error::StoreError::NoSpace)
        }
        _ => Ok(()),
    }
}
