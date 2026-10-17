//! Словарь терминов пользователя: хранение, поиск, экспорт и импорт файлом (TD-18, MVP-036).
//!
//! Словарь локальный. Обмен с коллегами — файлом `*.tolk-glossary.json`. Файл при импорте —
//! недоверенные данные: размер, схема и длины полей проверяются до любых изменений в базе.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};
use crate::jobs::now_ms;

pub const FORMAT_NAME: &str = "tolk-glossary";
pub const FORMAT_VERSION: u32 = 1;
/// Расширение файла экспорта.
pub const FILE_SUFFIX: &str = ".tolk-glossary.json";

pub const MAX_TERM_CHARS: usize = 100;
pub const MAX_VARIANT_CHARS: usize = 100;
pub const MAX_VARIANTS: usize = 20;
pub const MAX_NOTE_CHARS: usize = 500;
pub const MAX_TERMS: usize = 2_000;
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub id: i64,
    pub term: String,
    pub variants: Vec<String>,
    pub note: String,
    pub active: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Поля термина при создании и изменении.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TermInput {
    pub term: String,
    #[serde(default)]
    pub variants: Vec<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default = "default_active")]
    pub active: bool,
}

fn default_active() -> bool {
    true
}

/// Нормализованный ключ для сравнения: нижний регистр, `ё` = `е`, пробелы схлопнуты.
pub fn normalize_key(text: &str) -> String {
    text.to_lowercase()
        .replace('ё', "е")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn invalid(msg: impl Into<String>) -> StoreError {
    StoreError::GlossaryInvalid(msg.into())
}

/// Проверяет и приводит поля к виду для хранения: пробелы схлопнуты, варианты без дублей и без самого термина.
fn validate(input: &TermInput) -> Result<TermInput> {
    let term = clean(&input.term);
    if term.is_empty() {
        return Err(invalid("Термин не может быть пустым"));
    }
    if term.chars().count() > MAX_TERM_CHARS {
        return Err(invalid(format!(
            "Термин «{}…» длиннее {MAX_TERM_CHARS} символов",
            term.chars().take(20).collect::<String>()
        )));
    }
    if term.chars().any(char::is_control) || input.note.chars().any(|c| c.is_control() && c != '\n')
    {
        return Err(invalid(
            "Термин и заметка не должны содержать управляющих символов",
        ));
    }
    let note = input.note.trim().to_string();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(invalid(format!(
            "Заметка к термину «{term}» длиннее {MAX_NOTE_CHARS} символов"
        )));
    }
    if input.variants.len() > MAX_VARIANTS {
        return Err(invalid(format!(
            "У термина «{term}» больше {MAX_VARIANTS} вариантов"
        )));
    }
    let own_key = normalize_key(&term);
    let mut seen = HashSet::from([own_key]);
    let mut variants = Vec::new();
    for v in &input.variants {
        let v = clean(v);
        if v.is_empty() {
            continue;
        }
        if v.chars().count() > MAX_VARIANT_CHARS || v.chars().any(char::is_control) {
            return Err(invalid(format!("Некорректный вариант у термина «{term}»")));
        }
        if seen.insert(normalize_key(&v)) {
            variants.push(v);
        }
    }
    Ok(TermInput {
        term,
        variants,
        note,
        active: input.active,
    })
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<Term> {
    let variants: String = row.get(3)?;
    Ok(Term {
        id: row.get(0)?,
        term: row.get(1)?,
        // Поле пишет только этот модуль; при порче вариантов термин остаётся пригодным.
        variants: serde_json::from_str(&variants).unwrap_or_default(),
        note: row.get(4)?,
        active: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

const COLUMNS: &str = "id, term, term_key, variants, note, active, created_at, updated_at";

pub fn get(conn: &Connection, id: i64) -> Result<Option<Term>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM glossary_terms WHERE id = ?1"),
            [id],
            from_row,
        )
        .optional()?)
}

fn find_by_key(conn: &Connection, key: &str) -> Result<Option<Term>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM glossary_terms WHERE term_key = ?1"),
            [key],
            from_row,
        )
        .optional()?)
}

/// Все термины по алфавиту; `query` — поиск по термину и вариантам (без учёта регистра и `ё`).
pub fn list(conn: &Connection, query: Option<&str>) -> Result<Vec<Term>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM glossary_terms ORDER BY term_key"
    ))?;
    let all = stmt
        .query_map([], from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let needle = query.map(normalize_key).filter(|q| !q.is_empty());
    Ok(match needle {
        None => all,
        Some(q) => all
            .into_iter()
            .filter(|t| {
                normalize_key(&t.term).contains(&q)
                    || t.variants.iter().any(|v| normalize_key(v).contains(&q))
            })
            .collect(),
    })
}

/// Активные термины: именно они используются при распознавании (MVP-037).
pub fn active_terms(conn: &Connection) -> Result<Vec<Term>> {
    Ok(list(conn, None)?.into_iter().filter(|t| t.active).collect())
}

pub fn count(conn: &Connection) -> Result<usize> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM glossary_terms", [], |r| {
            r.get::<_, i64>(0)
        })? as usize,
    )
}

pub fn add(conn: &Connection, input: &TermInput) -> Result<Term> {
    let input = validate(input)?;
    let key = normalize_key(&input.term);
    if find_by_key(conn, &key)?.is_some() {
        return Err(StoreError::GlossaryDuplicate(input.term));
    }
    if count(conn)? >= MAX_TERMS {
        return Err(invalid(format!(
            "В словаре не может быть больше {MAX_TERMS} терминов"
        )));
    }
    insert_validated(conn, &input, &key)
}

fn insert_validated(conn: &Connection, input: &TermInput, key: &str) -> Result<Term> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO glossary_terms (term, term_key, variants, note, active, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        rusqlite::params![input.term, key, serde_json::to_string(&input.variants).unwrap_or_else(|_| "[]".into()), input.note, input.active, now],
    )?;
    get(conn, conn.last_insert_rowid())?.ok_or(StoreError::GlossaryNotFound)
}

pub fn update(conn: &Connection, id: i64, input: &TermInput) -> Result<Term> {
    let input = validate(input)?;
    let key = normalize_key(&input.term);
    if let Some(other) = find_by_key(conn, &key)? {
        if other.id != id {
            return Err(StoreError::GlossaryDuplicate(other.term));
        }
    }
    let changed = conn.execute(
        "UPDATE glossary_terms SET term = ?1, term_key = ?2, variants = ?3, note = ?4, active = ?5, updated_at = ?6 WHERE id = ?7",
        rusqlite::params![input.term, key, serde_json::to_string(&input.variants).unwrap_or_else(|_| "[]".into()), input.note, input.active, now_ms(), id],
    )?;
    if changed == 0 {
        return Err(StoreError::GlossaryNotFound);
    }
    get(conn, id)?.ok_or(StoreError::GlossaryNotFound)
}

pub fn set_active(conn: &Connection, id: i64, active: bool) -> Result<()> {
    let changed = conn.execute(
        "UPDATE glossary_terms SET active = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![active, now_ms(), id],
    )?;
    if changed == 0 {
        Err(StoreError::GlossaryNotFound)
    } else {
        Ok(())
    }
}

pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.execute("DELETE FROM glossary_terms WHERE id = ?1", [id])? > 0)
}

// ---- экспорт и импорт ----------------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GlossaryFile {
    format: String,
    version: u32,
    #[serde(default)]
    exported_at: String,
    terms: Vec<TermInput>,
}

/// Сводка импорта.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    /// Новые термины.
    pub added: usize,
    /// Существующие термины, к которым добавились варианты или заметка.
    pub merged: usize,
    /// Термины, которые уже есть в словаре без изменений (в том числе повторы внутри файла).
    pub skipped: usize,
}

/// UTC в формате ISO 8601 (`2026-10-18T09:30:00Z`) по Unix-времени в миллисекундах.
pub fn iso8601_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Дата по числу дней от 1970-01-01 (алгоритм civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Содержимое файла экспорта (все термины, включая неактивные).
pub fn export_json(conn: &Connection) -> Result<Vec<u8>> {
    let terms = list(conn, None)?
        .into_iter()
        .map(|t| TermInput {
            term: t.term,
            variants: t.variants,
            note: t.note,
            active: t.active,
        })
        .collect();
    let file = GlossaryFile {
        format: FORMAT_NAME.into(),
        version: FORMAT_VERSION,
        exported_at: iso8601_utc(now_ms()),
        terms,
    };
    serde_json::to_vec_pretty(&file)
        .map_err(|e| invalid(format!("Не удалось сформировать файл словаря: {e}")))
}

/// Разбирает и полностью проверяет файл словаря. Ничего не меняет.
fn parse_import(bytes: &[u8]) -> Result<Vec<TermInput>> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(invalid(format!(
            "Файл словаря больше {} КБ",
            MAX_FILE_BYTES / 1024
        )));
    }
    let file: GlossaryFile = serde_json::from_slice(bytes).map_err(|_| {
        invalid("Это не файл словаря dev-tolk: содержимое не соответствует формату")
    })?;
    if file.format != FORMAT_NAME {
        return Err(invalid("Это не файл словаря dev-tolk: неверный формат"));
    }
    if file.version != FORMAT_VERSION {
        return Err(invalid(format!(
            "Версия формата словаря {} не поддерживается",
            file.version
        )));
    }
    if file.terms.len() > MAX_TERMS {
        return Err(invalid(format!("В файле больше {MAX_TERMS} терминов")));
    }
    file.terms.iter().map(validate).collect()
}

/// Импорт словаря из файла со слиянием. Файл проверяется целиком до изменений; при любой ошибке
/// словарь остаётся прежним. Каноническое написание, заметка и флаг активности существующих терминов
/// не перезаписываются: к ним добавляются только новые варианты (и заметка, если своей нет).
pub fn import_json(conn: &mut Connection, bytes: &[u8]) -> Result<ImportSummary> {
    let incoming = parse_import(bytes)?;
    let tx = conn.transaction()?;
    let mut summary = ImportSummary::default();
    let mut total = count(&tx)?;
    for item in incoming {
        let key = normalize_key(&item.term);
        match find_by_key(&tx, &key)? {
            None => {
                if total >= MAX_TERMS {
                    return Err(invalid(format!(
                        "В словаре не может быть больше {MAX_TERMS} терминов"
                    )));
                }
                insert_validated(&tx, &item, &key)?;
                total += 1;
                summary.added += 1;
            }
            Some(existing) => {
                let mut known: HashSet<String> =
                    existing.variants.iter().map(|v| normalize_key(v)).collect();
                known.insert(key);
                let mut variants = existing.variants.clone();
                for v in item.variants {
                    if known.insert(normalize_key(&v)) {
                        variants.push(v);
                    }
                }
                let note_added = existing.note.is_empty() && !item.note.is_empty();
                if variants.len() > MAX_VARIANTS {
                    variants.truncate(MAX_VARIANTS);
                }
                if variants == existing.variants && !note_added {
                    summary.skipped += 1;
                    continue;
                }
                let note = if note_added {
                    item.note
                } else {
                    existing.note.clone()
                };
                tx.execute(
                    "UPDATE glossary_terms SET variants = ?1, note = ?2, updated_at = ?3 WHERE id = ?4",
                    rusqlite::params![serde_json::to_string(&variants).unwrap_or_else(|_| "[]".into()), note, now_ms(), existing.id],
                )?;
                summary.merged += 1;
            }
        }
    }
    tx.commit()?;
    Ok(summary)
}
