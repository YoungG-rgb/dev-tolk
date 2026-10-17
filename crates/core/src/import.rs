//! Импорт файла в очередь (MVP-018): проверка, `probe`, место на диске, копирование аудио или извлечение
//! звука из видео, создание задания.
//!
//! Порядок важен: файлы кладутся в `jobs/<id>/` **до** создания записи в базе, поэтому исполнитель очереди
//! не может взять задание без аудио. При любой ошибке каталог задания удаляется, записи нет; остатки
//! после сбоя на середине (`*.part`, каталог без записи) убирает очистка при старте (MVP-015).
//!
//! Видео не хранится: из него извлекается дорожка в WAV 16 кГц моно (Q-15, TD-09), исходник только читается.
//! Ошибки возвращаются как [`UserMessage`] с русским текстом и действием (MVP-025).

use std::ffi::OsString;
use std::path::Path;

use serde::Deserialize;

use crate::disk;
use crate::download::CancelToken;
use crate::engine::{self, ErrorInfo};
use crate::jobs::{self, Job, JobStatus, NewJob};
use crate::store::Store;
use crate::user_errors::{self, UserMessage};

/// WAV 16 кГц, моно, 16 бит: байт на секунду (для оценки нужного места).
const WAV_BYTES_PER_SECOND: u64 = 32_000;
/// Имя извлечённого аудио в каталоге задания.
pub const EXTRACTED_NAME: &str = "source.wav";

/// Расширения для диалога выбора файлов (определение формата идёт по содержимому, не по расширению).
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "m4a", "wav", "flac", "ogg", "oga", "opus", "webm", "aac",
];
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v"];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProbeInfo {
    pub container: String,
    pub codec: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub has_video: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ExtractDone {
    pub duration_ms: u64,
    #[serde(default)]
    pub incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ToolEvent {
    Probe(ProbeInfo),
    Progress { percent: u8 },
    Done(ExtractDone),
    Error(ErrorInfo),
}

/// Предупреждение: файл принят, но пользователю стоит знать.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug)]
pub struct Imported {
    pub job: Job,
    pub warnings: Vec<Warning>,
}

fn status_text(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "в очереди",
        JobStatus::Processing => "обрабатывается",
        JobStatus::Completed => "уже обработана",
        JobStatus::Failed => "обработка не удалась",
        JobStatus::Cancelled => "обработка отменена",
    }
}

fn cancelled() -> UserMessage {
    user_errors::from_engine("CANCELLED", None)
}

fn engine_failure(info: &ErrorInfo) -> UserMessage {
    user_errors::from_engine(&info.code, info.codec.as_deref())
}

fn internal(technical: impl std::fmt::Display) -> UserMessage {
    log::error!("Импорт: внутренняя ошибка: {technical}");
    user_errors::from_engine("INTERNAL", None)
}

/// Читает сведения о файле командой `probe` (звук не декодируется).
pub fn probe(exe: &Path, path: &Path, cancel: &CancelToken) -> Result<ProbeInfo, UserMessage> {
    let args: Vec<OsString> = vec!["probe".into(), path.as_os_str().to_owned()];
    let mut info = None;
    let mut error = None;
    let raw = engine::run_lines(
        exe,
        &args,
        None,
        cancel,
        &mut |line| match serde_json::from_str::<ToolEvent>(line.trim()) {
            Ok(ToolEvent::Probe(p)) => info = Some(p),
            Ok(ToolEvent::Error(e)) => error = Some(e),
            _ => {}
        },
    )
    .map_err(internal)?;
    if raw.cancel_requested {
        return Err(cancelled());
    }
    match (info, error) {
        (Some(p), _) => Ok(p),
        (None, Some(e)) => Err(engine_failure(&e)),
        (None, None) => {
            log::error!(
                "Импорт: probe завершился без результата (код {:?})",
                raw.exit_code
            );
            Err(user_errors::from_engine("INTERNAL", None))
        }
    }
}

/// Извлекает аудио в WAV 16 кГц моно. `on_progress` получает проценты по длительности входа.
pub fn extract(
    exe: &Path,
    input: &Path,
    output: &Path,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(u8),
) -> Result<ExtractDone, UserMessage> {
    let args: Vec<OsString> = vec![
        "extract".into(),
        input.as_os_str().to_owned(),
        output.as_os_str().to_owned(),
    ];
    let mut done = None;
    let mut error = None;
    let raw = engine::run_lines(
        exe,
        &args,
        None,
        cancel,
        &mut |line| match serde_json::from_str::<ToolEvent>(line.trim()) {
            Ok(ToolEvent::Progress { percent }) => on_progress(percent),
            Ok(ToolEvent::Done(d)) => done = Some(d),
            Ok(ToolEvent::Error(e)) => error = Some(e),
            _ => {}
        },
    )
    .map_err(internal)?;
    if raw.cancel_requested && done.is_none() {
        return Err(cancelled());
    }
    match (done, error) {
        (Some(d), _) => Ok(d),
        (None, Some(e)) => Err(engine_failure(&e)),
        (None, None) => {
            log::error!(
                "Импорт: extract завершился без результата (код {:?})",
                raw.exit_code
            );
            Err(user_errors::from_engine("INTERNAL", None))
        }
    }
}

/// Расширение для копии исходного файла: только латиница и цифры, до 8 знаков.
fn stored_extension(src: &Path) -> String {
    src.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "bin".to_string())
}

fn io_failure(e: &std::io::Error) -> UserMessage {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => user_errors::from_engine("FILE_NOT_FOUND", None),
        PermissionDenied => user_errors::from_engine("ACCESS_DENIED", None),
        _ => internal(e),
    }
}

/// Импортирует один файл. `on_progress` вызывается при извлечении звука из видео (0–100).
pub fn import_file(
    store: &Store,
    exe: &Path,
    src: &Path,
    model_id: Option<&str>,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(u8),
) -> Result<Imported, UserMessage> {
    let source_name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "запись".to_string());

    let meta = std::fs::metadata(src).map_err(|e| io_failure(&e))?;
    if !meta.is_file() {
        return Err(user_errors::from_engine("UNSUPPORTED_FORMAT", None));
    }
    // Доступность на чтение проверяем заранее, чтобы получить «нет доступа», а не общую ошибку.
    std::fs::File::open(src).map_err(|e| io_failure(&e))?;
    if meta.len() == 0 {
        return Err(user_errors::from_engine("CORRUPT_FILE", None));
    }

    let info = probe(exe, src, cancel)?;
    let duration_ms = info.duration_ms;

    let needed = if info.has_video {
        duration_ms.map_or(meta.len(), |d| d / 1000 * WAV_BYTES_PER_SECOND)
    } else {
        meta.len()
    };
    let layout = store.files.layout();
    std::fs::create_dir_all(layout.jobs_dir()).map_err(|e| io_failure(&e))?;
    disk::ensure_space(&layout.jobs_dir(), needed).map_err(|e| user_errors::from_store(&e))?;

    if cancel.is_cancelled() {
        return Err(cancelled());
    }

    let job_id = uuid::Uuid::new_v4().to_string();
    let result = (|| -> Result<(String, Option<i64>, Vec<Warning>), UserMessage> {
        let dir = store
            .files
            .ensure_job_dir(&job_id)
            .map_err(|e| user_errors::from_store(&e))?;
        let mut warnings = Vec::new();
        if info.has_video {
            let done = extract(exe, src, &dir.join(EXTRACTED_NAME), cancel, on_progress)?;
            if done.incomplete {
                warnings.push(Warning {
                    code: "INCOMPLETE",
                    message: "Файл оборван или частично повреждён: будет распознана только прочитанная часть.".into(),
                });
            }
            Ok((
                EXTRACTED_NAME.to_string(),
                Some(done.duration_ms as i64),
                warnings,
            ))
        } else {
            let name = format!("source.{}", stored_extension(src));
            store
                .files
                .copy_audio(&job_id, src, &name)
                .map_err(|e| user_errors::from_store(&e))?;
            Ok((name, duration_ms.map(|d| d as i64), warnings))
        }
    })();
    let (audio_name, duration, mut warnings) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = store.files.delete_job_dir(&job_id);
            return Err(e);
        }
    };
    if cancel.is_cancelled() {
        let _ = store.files.delete_job_dir(&job_id);
        return Err(cancelled());
    }

    let conn = store.db.conn();
    let similar = jobs::find_similar(&conn, &source_name, duration)
        .ok()
        .flatten();
    let job = jobs::insert_with_id(
        &conn,
        &job_id,
        &NewJob {
            source_name,
            audio_path: Some(audio_name),
            duration_ms: duration,
            model_id: model_id.map(str::to_string),
        },
    )
    .map_err(|e| {
        let _ = store.files.delete_job_dir(&job_id);
        user_errors::from_store(&e)
    })?;
    if let Some(status) = similar {
        warnings.insert(
            0,
            Warning {
                code: "DUPLICATE",
                message: format!(
                    "Запись с таким именем и длительностью уже есть в списке ({}). Она добавлена ещё раз.",
                    status_text(status)
                ),
            },
        );
    }
    Ok(Imported { job, warnings })
}

/// События пакетного импорта для интерфейса. Имена файлов нужны для сообщений пользователю; в журнал
/// приложения они не пишутся.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ImportEvent {
    #[serde(rename_all = "camelCase")]
    ImportStarted {
        index: usize,
        total: usize,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    ImportProgress { index: usize, percent: u8 },
    #[serde(rename_all = "camelCase")]
    ImportAccepted {
        index: usize,
        name: String,
        job_id: String,
        warnings: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    ImportRejected {
        index: usize,
        name: String,
        code: String,
        message: String,
        action: String,
    },
    #[serde(rename_all = "camelCase")]
    ImportBatchDone {
        total: usize,
        accepted: usize,
        rejected: usize,
        cancelled: bool,
    },
    /// Новый пакет пришёл, пока идёт предыдущий.
    ImportBusy,
}

/// Импортирует файлы по очереди. Отклонённый файл не мешает остальным; отмена останавливает пакет.
/// `after_each` вызывается после каждого принятого файла (например, чтобы разбудить исполнитель очереди).
pub fn import_many(
    store: &Store,
    exe: &Path,
    paths: &[std::path::PathBuf],
    model_id: Option<&str>,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(ImportEvent),
    after_each: &mut dyn FnMut(&Job),
) {
    let total = paths.len();
    let (mut accepted, mut rejected, mut was_cancelled) = (0, 0, false);
    for (index, path) in paths.iter().enumerate() {
        if cancel.is_cancelled() {
            was_cancelled = true;
            break;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "запись".to_string());
        on_event(ImportEvent::ImportStarted {
            index,
            total,
            name: name.clone(),
        });
        let mut last = None;
        let result = import_file(store, exe, path, model_id, cancel, &mut |percent| {
            if last != Some(percent) {
                last = Some(percent);
                on_event(ImportEvent::ImportProgress { index, percent });
            }
        });
        match result {
            Ok(imported) => {
                accepted += 1;
                after_each(&imported.job);
                on_event(ImportEvent::ImportAccepted {
                    index,
                    name,
                    job_id: imported.job.id,
                    warnings: imported.warnings.into_iter().map(|w| w.message).collect(),
                });
            }
            Err(e) if e.code == "CANCELLED" => {
                was_cancelled = true;
                break;
            }
            Err(e) => {
                rejected += 1;
                on_event(ImportEvent::ImportRejected {
                    index,
                    name,
                    code: e.code,
                    message: e.message,
                    action: e.action,
                });
            }
        }
    }
    on_event(ImportEvent::ImportBatchDone {
        total,
        accepted,
        rejected,
        cancelled: was_cancelled,
    });
}
