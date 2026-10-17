//! Правка транскрипта (MVP-020, TD-11): `original.json` не меняется, пользователь правит `transcript.json`.
//!
//! Правится только текст сегментов: границы времени берутся из файла на диске, поэтому интерфейс не может
//! их испортить. Запись атомарная (`JobFiles::write_atomic`): при сбое остаётся прежняя версия целой.
//! `has_edits` в базе — «рабочая копия отличается от оригинала».

use rusqlite::Connection;

use crate::error::{Result, StoreError};
use crate::jobs::{self, JobStatus};
use crate::store::Store;
use crate::transcript::{Segment, Transcript};

const ORIGINAL: &str = "original.json";
const WORKING: &str = "transcript.json";
/// Предел длины текста одного сегмента (защита от случайной вставки гигантского текста).
pub const MAX_SEGMENT_CHARS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditView {
    pub title: String,
    pub model: String,
    pub duration_ms: u64,
    pub has_edits: bool,
    pub segments: Vec<Segment>,
}

fn completed_job(conn: &Connection, id: &str) -> Result<jobs::Job> {
    let job = jobs::get(conn, id)?.ok_or_else(|| StoreError::JobNotFound(id.to_string()))?;
    if job.status != JobStatus::Completed {
        return Err(StoreError::InvalidInput(
            "Расшифровка ещё не готова.".into(),
        ));
    }
    Ok(job)
}

fn read_transcript(store: &Store, id: &str, name: &str) -> Result<Option<Transcript>> {
    store
        .files
        .read(id, name)?
        .map(|b| Transcript::from_json(&b))
        .transpose()
}

/// Рабочая копия, а при её отсутствии (или порче) — оригинал.
fn read_working(store: &Store, id: &str) -> Result<Transcript> {
    match read_transcript(store, id, WORKING) {
        Ok(Some(t)) => return Ok(t),
        Ok(None) => {}
        Err(e) => log::warn!("Рабочая копия расшифровки не читается, берётся оригинал: {e}"),
    }
    read_transcript(store, id, ORIGINAL)?
        .ok_or_else(|| StoreError::BadTranscript("файл расшифровки не найден".into()))
}

fn differs(working: &Transcript, original: &Transcript) -> bool {
    working.segments.len() != original.segments.len()
        || working
            .segments
            .iter()
            .zip(&original.segments)
            .any(|(a, b)| a.text != b.text)
}

fn view(job: jobs::Job, t: Transcript, has_edits: bool) -> EditView {
    EditView {
        title: job.source_name,
        model: t.model,
        duration_ms: t.duration_ms,
        has_edits,
        segments: t.segments,
    }
}

pub fn load(store: &Store, id: &str) -> Result<EditView> {
    let job = completed_job(&store.db.conn(), id)?;
    let t = read_working(store, id)?;
    let has_edits = job.has_edits;
    Ok(view(job, t, has_edits))
}

/// Сохраняет тексты сегментов (по одному на сегмент, в том же порядке). Возвращает новое значение `has_edits`.
pub fn save_texts(store: &Store, id: &str, texts: &[String]) -> Result<bool> {
    completed_job(&store.db.conn(), id)?;
    let mut working = read_working(store, id)?;
    if texts.len() != working.segments.len() {
        return Err(StoreError::InvalidInput(
            "Число фрагментов текста не совпадает с расшифровкой.".into(),
        ));
    }
    if texts.iter().any(|t| t.chars().count() > MAX_SEGMENT_CHARS) {
        return Err(StoreError::InvalidInput(
            "Фрагмент текста слишком длинный.".into(),
        ));
    }
    for (segment, text) in working.segments.iter_mut().zip(texts) {
        segment.text = text.clone();
    }
    let has_edits = match read_transcript(store, id, ORIGINAL)? {
        Some(original) => differs(&working, &original),
        None => true,
    };
    store.files.write_atomic(id, WORKING, &working.to_json()?)?;
    jobs::set_has_edits(&store.db.conn(), id, has_edits)?;
    Ok(has_edits)
}

/// «Вернуть исходный текст»: рабочая копия снова равна оригиналу.
pub fn revert(store: &Store, id: &str) -> Result<EditView> {
    let job = completed_job(&store.db.conn(), id)?;
    let bytes = store
        .files
        .read(id, ORIGINAL)?
        .ok_or_else(|| StoreError::BadTranscript("исходный файл расшифровки не найден".into()))?;
    let original = Transcript::from_json(&bytes)?;
    store.files.write_atomic(id, WORKING, &bytes)?;
    jobs::set_has_edits(&store.db.conn(), id, false)?;
    Ok(view(job, original, false))
}
