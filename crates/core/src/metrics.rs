//! Метрики производительности для замеров на устройствах сотрудников (Q-14, TD-14, MVP-038).
//!
//! По каждому заданию локально сохраняются длительность записи, время обработки, пиковая память движка,
//! бэкенд, модель и результат. Из них собирается файл отчёта: система и метрики, **без имён файлов,
//! текстов и идентификаторов заданий**. Сотрудник передаёт файл вручную, сетевой отправки нет.

use std::path::Path;

use rusqlite::Connection;
use serde::Serialize;

use crate::error::{Result, StoreError};
use crate::export::write_file_atomic;
use crate::jobs::now_ms;
use crate::models::SystemInfo;

pub const REPORT_FORMAT: &str = "tolk-performance-report";
pub const REPORT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failed,
    Cancelled,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "SUCCESS",
            Outcome::Failed => "FAILED",
            Outcome::Cancelled => "CANCELLED",
        }
    }
}

/// Данные события `done` движка (`docs/ENGINE_PROTOCOL.md`) и результат задания.
#[derive(Debug, Clone)]
pub struct JobMetrics {
    pub audio_ms: u64,
    pub processing_ms: u64,
    pub peak_rss_bytes: Option<u64>,
    /// `metal`, `vulkan` или `cpu`.
    pub backend: Option<String>,
    pub model_id: Option<String>,
    pub outcome: Outcome,
    /// Код ошибки при неуспехе.
    pub error_code: Option<String>,
}

/// Сохраняет метрики задания. Повторная запись для того же задания заменяет прежнюю (повтор после ошибки).
pub fn record(conn: &Connection, job_id: &str, m: &JobMetrics) -> Result<()> {
    conn.execute(
        "INSERT INTO job_metrics (job_id, audio_ms, processing_ms, peak_rss_bytes, backend, model_id, outcome, error_code, recorded_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(job_id) DO UPDATE SET audio_ms = excluded.audio_ms, processing_ms = excluded.processing_ms,
             peak_rss_bytes = excluded.peak_rss_bytes, backend = excluded.backend, model_id = excluded.model_id,
             outcome = excluded.outcome, error_code = excluded.error_code, recorded_at = excluded.recorded_at,
             user_rating = NULL",
        rusqlite::params![
            job_id,
            m.audio_ms as i64,
            m.processing_ms as i64,
            m.peak_rss_bytes.map(|v| v as i64),
            m.backend,
            m.model_id,
            m.outcome.as_str(),
            m.error_code,
            now_ms()
        ],
    )?;
    Ok(())
}

/// Оценка качества пользователем, 1–5.
pub fn set_rating(conn: &Connection, job_id: &str, rating: u8) -> Result<()> {
    if !(1..=5).contains(&rating) {
        return Err(StoreError::InvalidInput(
            "Оценка должна быть от 1 до 5".into(),
        ));
    }
    let changed = conn.execute(
        "UPDATE job_metrics SET user_rating = ?1 WHERE job_id = ?2",
        rusqlite::params![rating, job_id],
    )?;
    if changed == 0 {
        return Err(StoreError::JobNotFound(job_id.to_string()));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct SystemSection {
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub cpu: String,
    pub cpu_physical_cores: Option<usize>,
    pub ram_total_mb: u64,
    pub avx2: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct JobEntry {
    pub audio_s: f64,
    pub processing_s: f64,
    /// Время обработки к длительности записи.
    pub rtf: Option<f64>,
    pub peak_rss_mb: Option<u64>,
    pub backend: Option<String>,
    pub model: Option<String>,
    pub outcome: String,
    pub error_code: Option<String>,
    pub user_rating: Option<u8>,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub model: Option<String>,
    pub backend: Option<String>,
    pub jobs: usize,
    pub succeeded: usize,
    pub rtf_median: Option<f64>,
    pub rtf_max: Option<f64>,
    pub peak_rss_mb_max: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub format: &'static str,
    pub version: u32,
    pub app_version: String,
    pub generated_at: String,
    pub system: SystemSection,
    pub summaries: Vec<Summary>,
    pub jobs: Vec<JobEntry>,
}

fn median(sorted: &[f64]) -> Option<f64> {
    match sorted.len() {
        0 => None,
        n if n % 2 == 1 => Some(sorted[n / 2]),
        n => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2.0),
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Собирает отчёт. `app_version` и сведения о системе передаёт вызывающий код.
pub fn build_report(conn: &Connection, system: &SystemInfo, app_version: &str) -> Result<Report> {
    let mut stmt = conn.prepare(
        "SELECT audio_ms, processing_ms, peak_rss_bytes, backend, model_id, outcome, error_code, user_rating
         FROM job_metrics ORDER BY recorded_at, id",
    )?;
    let jobs: Vec<JobEntry> = stmt
        .query_map([], |row| {
            let audio_ms: i64 = row.get(0)?;
            let processing_ms: i64 = row.get(1)?;
            let peak: Option<i64> = row.get(2)?;
            Ok(JobEntry {
                audio_s: round3(audio_ms as f64 / 1000.0),
                processing_s: round3(processing_ms as f64 / 1000.0),
                rtf: (audio_ms > 0).then(|| round3(processing_ms as f64 / audio_ms as f64)),
                peak_rss_mb: peak.map(|b| (b as u64).div_ceil(1024 * 1024)),
                backend: row.get(3)?,
                model: row.get(4)?,
                outcome: row.get(5)?,
                error_code: row.get(6)?,
                user_rating: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut groups: Vec<(Option<String>, Option<String>)> = Vec::new();
    for j in &jobs {
        let key = (j.model.clone(), j.backend.clone());
        if !groups.contains(&key) {
            groups.push(key);
        }
    }
    let summaries = groups
        .into_iter()
        .map(|(model, backend)| {
            let members: Vec<&JobEntry> = jobs
                .iter()
                .filter(|j| j.model == model && j.backend == backend)
                .collect();
            let mut rtfs: Vec<f64> = members
                .iter()
                .filter(|j| j.outcome == "SUCCESS")
                .filter_map(|j| j.rtf)
                .collect();
            rtfs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            Summary {
                model,
                backend,
                jobs: members.len(),
                succeeded: members.iter().filter(|j| j.outcome == "SUCCESS").count(),
                rtf_median: median(&rtfs).map(round3),
                rtf_max: rtfs.last().copied(),
                peak_rss_mb_max: members.iter().filter_map(|j| j.peak_rss_mb).max(),
            }
        })
        .collect();

    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_all();
    Ok(Report {
        format: REPORT_FORMAT,
        version: REPORT_VERSION,
        app_version: app_version.to_string(),
        generated_at: crate::glossary::iso8601_utc(now_ms()),
        system: SystemSection {
            os: system.os.to_string(),
            os_version: sysinfo::System::long_os_version().unwrap_or_default(),
            arch: system.arch.to_string(),
            cpu: sys
                .cpus()
                .first()
                .map(|c| c.brand().trim().to_string())
                .unwrap_or_default(),
            cpu_physical_cores: sysinfo::System::physical_core_count(),
            ram_total_mb: system.total_ram_bytes / (1024 * 1024),
            avx2: system.avx2,
        },
        summaries,
        jobs,
    })
}

/// Сохраняет отчёт в файл (кнопка «Сохранить отчёт о производительности»).
pub fn save_report(
    conn: &Connection,
    system: &SystemInfo,
    app_version: &str,
    dest: &Path,
) -> Result<()> {
    let report = build_report(conn, system, app_version)?;
    let bytes =
        serde_json::to_vec_pretty(&report).map_err(|e| StoreError::Export(e.to_string()))?;
    write_file_atomic(&bytes, dest)
}
