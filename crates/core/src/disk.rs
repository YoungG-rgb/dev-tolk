//! Свободное место на диске (загрузка моделей, импорт файлов, экспорт).

use std::path::Path;

/// Сколько байт доступно пользователю на томе, где лежит `path` (каталог должен существовать).
pub fn available_bytes(path: &Path) -> std::io::Result<u64> {
    fs4::available_space(path)
}
