//! Формат транскрипта: `original.json` (неизменяемый результат движка) и `transcript.json` (рабочая версия).
//! Одна и та же структура; формат фиксируется в `docs/ENGINE_PROTOCOL.md` вместе с командой `transcribe` (MVP-009).

use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub version: u32,
    /// Движок и его версия, например `whisper.cpp 1.9.5`.
    pub engine: String,
    /// Название модели, например `large-v3-turbo-q5_0`.
    pub model: String,
    pub language: String,
    pub duration_ms: u64,
    pub segments: Vec<Segment>,
}

impl Transcript {
    /// Разбирает и проверяет транскрипт. Файл на диске — недоверенные данные: его могли править вручную.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let t: Transcript =
            serde_json::from_slice(bytes).map_err(|e| StoreError::BadTranscript(e.to_string()))?;
        if t.version != FORMAT_VERSION {
            return Err(StoreError::BadTranscript(format!(
                "версия формата {} не поддерживается",
                t.version
            )));
        }
        if let Some(bad) = t.segments.iter().position(|s| s.end_ms < s.start_ms) {
            return Err(StoreError::BadTranscript(format!(
                "сегмент {bad}: конец раньше начала"
            )));
        }
        Ok(t)
    }

    pub fn to_json(&self) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(self).map_err(|e| StoreError::BadTranscript(e.to_string()))
    }
}
