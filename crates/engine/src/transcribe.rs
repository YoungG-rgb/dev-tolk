//! Команда `transcribe`: русский текст с таймкодами из длинного файла (MVP-009).
//!
//! Конвейер: проверка модели → декодирование в моно 16 кГц → whisper.cpp (через `whisper-rs`) с VAD →
//! `original.json` атомарно. Параметры повторяют `whisper-cli` v1.9.5 для конфигурации W1 (BENCHMARK_PLAN §4.1):
//! язык `ru`, без контекста (`-mc 0`), beam search 5, flash attention, VAD Silero с параметрами по умолчанию.
//!
//! **Предварительно:** выбор движка и привязки (TD-02, TD-03, TD-05) подтверждается бенчмарком (MVP-007).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperError,
    WhisperVadParams,
};

use crate::decode;
use crate::error::{EngineError, ErrorCode, Result};
use crate::memory;

pub const FORMAT_VERSION: u32 = 1;
/// Версия движка для `original.json`: вендорится `whisper-rs` 0.16.
pub const ENGINE_NAME: &str = "whisper.cpp 1.8.3 (whisper-rs 0.16)";
/// Доля шкалы прогресса, отведённая на декодирование (остальное — распознавание).
const DECODE_SHARE: u32 = 4;
/// Потоков по умолчанию: не больше 4 (как в whisper.cpp) и не больше числа ядер. Больше потоков, чем ядер
/// (виртуальные машины CI с 3 ядрами), заметно замедляет декодирование: потоки ggml ждут друг друга активно.
fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(4))
}

#[derive(Debug, Clone)]
pub struct Args {
    pub model: PathBuf,
    pub vad_model: Option<PathBuf>,
    pub input: PathBuf,
    pub output: PathBuf,
    pub language: String,
    pub threads: Option<usize>,
    pub no_gpu: bool,
    /// Завершаться, когда родитель закрыл stdin (родитель погиб): дочерний процесс не остаётся висеть.
    pub exit_on_stdin_close: bool,
}

#[derive(Debug, Serialize)]
pub struct Timings {
    pub elapsed_ms: u64,
    pub model_load_ms: u64,
    pub decode_ms: u64,
    pub recognize_ms: u64,
}

#[derive(Debug)]
pub struct Outcome {
    pub segments: usize,
    pub audio_ms: u64,
    pub timings: Timings,
    pub peak_rss_bytes: Option<u64>,
    pub backend: &'static str,
}

#[derive(Serialize, Clone)]
struct Segment {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

/// Формат `original.json` (совпадает с `tolk-core::transcript::Transcript`).
#[derive(Serialize)]
struct OriginalJson {
    version: u32,
    engine: String,
    model: String,
    language: String,
    duration_ms: u64,
    segments: Vec<Segment>,
}

/// Идентификатор модели по имени файла: `ggml-small-q5_1.bin` → `small-q5_1`.
pub fn model_id_from_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    stem.strip_prefix("ggml-").unwrap_or(stem).to_string()
}

fn check_file(path: &Path, what: &str) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() > 0 => Ok(()),
        Ok(m) if m.is_file() => Err(EngineError::new(
            ErrorCode::ModelMissing,
            format!("{what}: файл пуст"),
        )),
        _ => Err(EngineError::new(
            ErrorCode::ModelMissing,
            format!("{what}: файл не найден"),
        )),
    }
}

fn model_error(err: &WhisperError) -> EngineError {
    EngineError::new(
        ErrorCode::ModelInvalid,
        format!("Не удалось загрузить модель: файл повреждён, не подходит или не хватает памяти ({err})"),
    )
}

/// Сигнал отмены (SIGTERM/SIGINT): распознавание прерывается корректно, итоговый файл не создаётся.
static CANCELLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn install_cancel_handler() {
    extern "C" fn on_signal(_: libc::c_int) {
        CANCELLED.store(true, Ordering::SeqCst);
    }
    // SAFETY: обработчик только сохраняет атомарный флаг.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as usize);
        libc::signal(libc::SIGINT, on_signal as *const () as usize);
    }
}

/// Сколько после сигнала отмены даём распознаванию остановиться самому, прежде чем завершить процесс.
#[cfg(unix)]
const CANCEL_GRACE: std::time::Duration = std::time::Duration::from_millis(1000);

/// Страховка отмены: whisper.cpp проверяет флаг прерывания только между узлами графа, а на CPU один узел
/// (кодировщик, свёртка, BLAS) может длиться секундами (на CPU виртуальной машины CI отмена занимала 15 с).
/// Если процесс не завершился за `CANCEL_GRACE`, сообщаем об отмене и выходим сами. Итоговый файл пишется
/// атомарно и только после распознавания, поэтому потерять и испортить при этом нечего.
#[cfg(unix)]
fn spawn_cancel_watchdog() {
    std::thread::spawn(|| {
        while !CANCELLED.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        std::thread::sleep(CANCEL_GRACE);
        crate::emit(&crate::Event::Error {
            code: ErrorCode::Cancelled,
            message: "Обработка отменена",
            codec: None,
        });
        // `_exit`, а не `exit`: обработчики atexit ggml при работающем распознавании падают сигналом.
        // SAFETY: немедленное завершение процесса без очистки.
        unsafe { libc::_exit(ErrorCode::Cancelled.exit_code()) }
    });
}

#[cfg(not(unix))]
fn install_cancel_handler() {}

fn check_cancelled() -> Result<()> {
    if CANCELLED.load(Ordering::SeqCst) {
        Err(EngineError::new(ErrorCode::Cancelled, "Обработка отменена"))
    } else {
        Ok(())
    }
}

/// Следит за stdin: EOF означает, что приложение закрыло канал или погибло. Итоговый файл пишется
/// атомарно, поэтому внезапное завершение не оставляет частичного результата.
fn watch_parent_stdin() {
    std::thread::spawn(|| {
        let mut sink = [0u8; 256];
        let mut stdin = std::io::stdin().lock();
        while matches!(std::io::Read::read(&mut stdin, &mut sink), Ok(n) if n > 0) {}
        // `_exit`, а не `exit`: обработчики atexit библиотеки ggml (Metal) при работающем распознавании падают сигналом.
        // SAFETY: немедленное завершение процесса без очистки; результат пишется атомарно, потерять нечего.
        #[cfg(unix)]
        unsafe {
            libc::_exit(143)
        }
        #[cfg(not(unix))]
        std::process::exit(143);
    });
}

pub fn run(args: &Args) -> Result<Outcome> {
    let started = Instant::now();
    install_cancel_handler();
    #[cfg(unix)]
    spawn_cancel_watchdog();
    if args.exit_on_stdin_close {
        watch_parent_stdin();
    }

    // Всё, что можно проверить до дорогой работы, проверяется до неё: ошибка модели не ждёт декодирования.
    check_file(&args.model, "Модель распознавания")?;
    if let Some(vad) = &args.vad_model {
        check_file(vad, "Модель определения речи")?;
    }
    let source = decode::open(&args.input)?;
    crate::print_progress(0);

    // 1. Декодирование.
    let decode_started = Instant::now();
    let mut audio: Vec<f32> = Vec::new();
    let mut last_decode_pct = 0;
    decode::decode_to_mono_16k(
        source,
        &mut |samples| {
            audio.extend_from_slice(samples);
            check_cancelled()
        },
        &mut |fraction| {
            let pct = ((fraction * f64::from(DECODE_SHARE)) as u32).min(DECODE_SHARE);
            if pct > last_decode_pct {
                last_decode_pct = pct;
                crate::print_progress(pct);
            }
        },
    )?;
    let decode_ms = decode_started.elapsed().as_millis() as u64;
    let audio_ms = audio.len() as u64 * 1000 / u64::from(decode::TARGET_RATE);
    if last_decode_pct < DECODE_SHARE {
        crate::print_progress(DECODE_SHARE);
    }

    // 2. Модель.
    let load_started = Instant::now();
    let mut ctx_params = WhisperContextParameters::default();
    ctx_params.use_gpu(!args.no_gpu);
    ctx_params.flash_attn(true);
    let model_path = args
        .model
        .to_str()
        .ok_or_else(|| EngineError::new(ErrorCode::ModelMissing, "Путь к модели не в UTF-8"))?;
    let ctx =
        WhisperContext::new_with_params(model_path, ctx_params).map_err(|e| model_error(&e))?;
    let mut state = ctx.create_state().map_err(|e| model_error(&e))?;
    let model_load_ms = load_started.elapsed().as_millis() as u64;
    check_cancelled()?;

    // 3. Распознавание. Прогресс и сегменты печатаются прямо из обратных вызовов whisper.cpp.
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    params.set_language(Some(&args.language));
    params.set_n_threads(args.threads.unwrap_or_else(default_threads).max(1) as i32);
    params.set_n_max_text_ctx(0); // как `-mc 0`: без контекста между окнами, меньше зацикливаний
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    if let Some(vad) = &args.vad_model {
        // whisper-rs требует задать путь к модели до включения VAD.
        params.set_vad_model_path(vad.to_str());
        params.enable_vad(true);
        params.set_vad_params(WhisperVadParams::default());
    }

    // Прогресс не убывает и не выходит за 99: 100 печатается только после записи результата.
    let progress = Arc::new(AtomicU32::new(DECODE_SHARE));
    params.set_progress_callback_safe(move |p: i32| {
        let mapped = DECODE_SHARE + (p.clamp(0, 100) as u32) * (99 - DECODE_SHARE) / 100;
        if progress.fetch_max(mapped, Ordering::SeqCst) < mapped {
            crate::print_progress(mapped);
        }
    });
    params.set_abort_callback_safe(|| CANCELLED.load(Ordering::SeqCst));

    let collected: Arc<Mutex<Vec<Segment>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    params.set_segment_callback_safe_lossy(move |data: whisper_rs::SegmentCallbackData| {
        // Таймкоды whisper.cpp — сотые доли секунды.
        let seg = Segment {
            start_ms: data.start_timestamp.max(0) as u64 * 10,
            end_ms: data.end_timestamp.max(0) as u64 * 10,
            text: data.text.trim().to_string(),
        };
        crate::print_segment(seg.start_ms, seg.end_ms, &seg.text);
        if let Ok(mut all) = sink.lock() {
            all.push(seg);
        }
    });

    let recognize_started = Instant::now();
    let result = state.full(params, &audio);
    if CANCELLED.load(Ordering::SeqCst) {
        return Err(EngineError::new(ErrorCode::Cancelled, "Обработка отменена"));
    }
    result
        .map_err(|e| EngineError::new(ErrorCode::Internal, format!("Ошибка распознавания: {e}")))?;
    let recognize_ms = recognize_started.elapsed().as_millis() as u64;

    // 4. Результат: сначала файл, потом сообщение об успехе.
    let segments = collected.lock().map(|s| s.clone()).unwrap_or_default();
    let original = OriginalJson {
        version: FORMAT_VERSION,
        engine: ENGINE_NAME.to_string(),
        model: model_id_from_path(&args.model),
        language: args.language.clone(),
        duration_ms: audio_ms,
        segments,
    };
    let bytes = serde_json::to_vec_pretty(&original).map_err(|e| {
        EngineError::new(
            ErrorCode::Internal,
            format!("Не удалось сформировать результат: {e}"),
        )
    })?;
    crate::atomic::write_atomic(&args.output, &bytes)?;
    crate::print_progress(100);

    Ok(Outcome {
        segments: original.segments.len(),
        audio_ms,
        timings: Timings {
            elapsed_ms: started.elapsed().as_millis() as u64,
            model_load_ms,
            decode_ms,
            recognize_ms,
        },
        peak_rss_bytes: memory::peak_memory_bytes(),
        backend: if args.no_gpu || !cfg!(target_os = "macos") {
            "cpu"
        } else {
            "metal"
        },
    })
}
