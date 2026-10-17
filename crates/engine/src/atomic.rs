//! Атомарная запись результата: временный файл рядом, сброс на диск, переименование (TD-09).

use std::io::Write;
use std::path::Path;

use tempfile::NamedTempFile;

use crate::error::{EngineError, Result};

/// Записывает `bytes` в `dest`: итоговый файл появляется только целиком.
pub fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<()> {
    let dir = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::Builder::new()
        .prefix(".tolk-result-")
        .suffix(".part")
        .tempfile_in(dir)
        .map_err(|e| EngineError::from_output_io(&e))?;
    tmp.write_all(bytes)
        .map_err(|e| EngineError::from_output_io(&e))?;
    finish(tmp, dest)
}

fn finish(tmp: NamedTempFile, dest: &Path) -> Result<()> {
    tmp.as_file()
        .sync_all()
        .map_err(|e| EngineError::from_output_io(&e))?;
    tmp.persist(dest)
        .map_err(|e| EngineError::from_output_io(&e.error))?;
    Ok(())
}
