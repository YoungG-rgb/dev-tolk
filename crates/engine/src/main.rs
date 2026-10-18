//! tolk-engine: отдельный процесс-движок dev-tolk (TD-04).
//!
//! Команды: `probe`, `extract` (MVP-008) и `transcribe` (MVP-009, предварительно). Протокол — JSON Lines в stdout,
//! описан в `docs/ENGINE_PROTOCOL.md`.

mod atomic;
mod decode;
mod error;
mod memory;
mod transcribe;
mod wav;

use std::ffi::OsString;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;

use crate::error::{EngineError, ErrorCode, Result};

const USAGE: &str = "\
tolk-engine — движок транскрибации dev-tolk

Использование:
  tolk-engine probe <файл>               сведения о файле (JSON)
  tolk-engine extract <файл> <out.wav>   аудио -> WAV 16 кГц моно 16 бит
  tolk-engine transcribe --model <модель> --input <файл> --output <original.json>
                         [--vad-model <модель>] [--language ru] [--threads N] [--no-gpu]
                         [--exit-on-stdin-close]
  tolk-engine --version
";

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event<'a> {
    Probe {
        container: &'a str,
        codec: &'a str,
        duration_ms: Option<u64>,
        sample_rate: Option<u32>,
        channels: Option<u32>,
        has_video: bool,
    },
    Progress {
        percent: u32,
    },
    Done {
        output: &'a str,
        duration_ms: u64,
        samples: u64,
        incomplete: bool,
        skipped_packets: u64,
    },
    Segment {
        start_ms: u64,
        end_ms: u64,
        text: &'a str,
    },
    #[serde(rename = "done")]
    TranscribeDone {
        output: &'a str,
        segments: usize,
        audio_ms: u64,
        elapsed_ms: u64,
        model_load_ms: u64,
        decode_ms: u64,
        recognize_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        peak_rss_bytes: Option<u64>,
        backend: &'a str,
    },
    Error {
        code: ErrorCode,
        message: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        codec: Option<&'a str>,
    },
}

fn emit(event: &Event<'_>) {
    let mut out = std::io::stdout().lock();
    // Ошибка записи в stdout (закрытый канал) не должна приводить к panic.
    let _ = serde_json::to_writer(&mut out, event);
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

/// Печать события прогресса из любого места (в том числе из обратного вызова whisper.cpp).
pub(crate) fn print_progress(percent: u32) {
    emit(&Event::Progress { percent });
}

/// Печать готового сегмента.
pub(crate) fn print_segment(start_ms: u64, end_ms: u64, text: &str) {
    emit(&Event::Segment {
        start_ms,
        end_ms,
        text,
    });
}

fn parse_transcribe_args(args: &[OsString]) -> Result<transcribe::Args> {
    let usage = |msg: String| EngineError::new(ErrorCode::Usage, format!("{msg}\n{USAGE}"));
    let (mut model, mut vad, mut input, mut output) = (None, None, None, None);
    let mut language = "ru".to_string();
    let mut threads = None;
    // Переменная окружения TOLK_NO_GPU=1 равна флагу `--no-gpu`: диагностический переключатель для
    // компьютеров и виртуальных машин, где драйвер GPU работает неверно или очень медленно.
    let mut no_gpu = std::env::var_os("TOLK_NO_GPU").is_some_and(|v| v != "0" && !v.is_empty());
    let mut exit_on_stdin_close = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let name = flag.to_str().unwrap_or("");
        let mut value = |what: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| usage(format!("{what}: не указано значение")))
        };
        match name {
            "--model" => model = Some(PathBuf::from(value("--model")?)),
            "--vad-model" => vad = Some(PathBuf::from(value("--vad-model")?)),
            "--input" => input = Some(PathBuf::from(value("--input")?)),
            "--output" => output = Some(PathBuf::from(value("--output")?)),
            "--language" => language = value("--language")?.to_string_lossy().into_owned(),
            "--threads" => {
                let raw = value("--threads")?;
                threads = Some(
                    raw.to_string_lossy()
                        .parse::<usize>()
                        .map_err(|_| usage("--threads: нужно целое число".into()))?,
                );
            }
            "--no-gpu" => no_gpu = true,
            // Приложение держит открытым канал stdin: если оно погибло, канал закрывается и движок завершается сам.
            "--exit-on-stdin-close" => exit_on_stdin_close = true,
            other => return Err(usage(format!("transcribe: неизвестный аргумент {other}"))),
        }
    }
    Ok(transcribe::Args {
        model: model.ok_or_else(|| usage("transcribe: нужен --model".into()))?,
        vad_model: vad,
        input: input.ok_or_else(|| usage("transcribe: нужен --input".into()))?,
        output: output.ok_or_else(|| usage("transcribe: нужен --output".into()))?,
        language,
        threads,
        no_gpu,
        exit_on_stdin_close,
    })
}

fn cmd_transcribe(args: &[OsString]) -> Result<()> {
    let args = parse_transcribe_args(args)?;
    if same_file(&args.input, &args.output) {
        return Err(EngineError::new(
            ErrorCode::Usage,
            "Выходной файл совпадает с входным: исходный файл не изменяется",
        ));
    }
    let outcome = transcribe::run(&args)?;
    emit(&Event::TranscribeDone {
        output: &args.output.to_string_lossy(),
        segments: outcome.segments,
        audio_ms: outcome.audio_ms,
        elapsed_ms: outcome.timings.elapsed_ms,
        model_load_ms: outcome.timings.model_load_ms,
        decode_ms: outcome.timings.decode_ms,
        recognize_ms: outcome.timings.recognize_ms,
        peak_rss_bytes: outcome.peak_rss_bytes,
        backend: outcome.backend,
    });
    Ok(())
}

fn cmd_probe(path: &Path) -> Result<()> {
    let source = decode::open(path)?;
    let mut info = source.info.clone();
    if info.duration_ms.is_none() {
        info.duration_ms = source.duration_ms_or_scan()?;
    }
    emit(&Event::Probe {
        container: info.container,
        codec: &info.codec,
        duration_ms: info.duration_ms,
        sample_rate: info.sample_rate,
        channels: info.channels,
        has_video: info.has_video,
    });
    Ok(())
}

fn cmd_extract(input: &Path, output: &Path) -> Result<()> {
    if same_file(input, output) {
        return Err(EngineError::new(
            ErrorCode::Usage,
            "Выходной файл совпадает с входным: исходный файл не изменяется",
        ));
    }
    let source = decode::open(input)?;
    let mut writer = wav::WavWriter::create(output)?;
    let mut last_percent = 0u32;
    emit(&Event::Progress { percent: 0 });

    let stats = decode::decode_to_mono_16k(
        source,
        &mut |samples| writer.write_f32(samples),
        &mut |fraction| {
            let percent = ((fraction * 100.0) as u32).min(99);
            if percent > last_percent {
                last_percent = percent;
                emit(&Event::Progress { percent });
            }
        },
    )?;
    writer.finish(output)?;

    emit(&Event::Progress { percent: 100 });
    emit(&Event::Done {
        output: &output.to_string_lossy(),
        duration_ms: stats.frames * 1000 / u64::from(decode::TARGET_RATE),
        samples: stats.frames,
        incomplete: stats.incomplete,
        skipped_packets: stats.skipped_packets,
    });
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn run(args: Vec<OsString>) -> Result<()> {
    let usage = |msg: &str| EngineError::new(ErrorCode::Usage, format!("{msg}\n{USAGE}"));
    let mut it = args.into_iter();
    let Some(cmd) = it.next() else {
        return Err(usage("Не указана команда"));
    };
    let raw: Vec<OsString> = it.collect();
    if cmd.to_str() == Some("transcribe") {
        return cmd_transcribe(&raw);
    }
    let rest: Vec<PathBuf> = raw.into_iter().map(PathBuf::from).collect();
    match (cmd.to_str(), rest.as_slice()) {
        (Some("probe"), [file]) => cmd_probe(file),
        (Some("extract"), [input, output]) => cmd_extract(input, output),
        (Some("probe"), _) => Err(usage("probe: ожидается один аргумент — файл")),
        (Some("extract"), _) => Err(usage("extract: ожидаются два аргумента — файл и out.wav")),
        (Some("--version" | "-V"), _) => {
            println!("tolk-engine {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        (Some("--help" | "-h"), _) => {
            print!("{USAGE}");
            Ok(())
        }
        _ => Err(usage("Неизвестная команда")),
    }
}

fn main() -> ExitCode {
    // Panic во внешней библиотеке (например, в демультиплексоре MP4) не должен
    // выводить трассировку: сообщаем ошибкой и выходим с кодом внутренней ошибки.
    panic::set_hook(Box::new(|info| {
        eprintln!("tolk-engine: внутренняя ошибка: {info}");
    }));

    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let result = panic::catch_unwind(AssertUnwindSafe(|| run(args)));
    let err = match result {
        Ok(Ok(())) => return ExitCode::SUCCESS,
        Ok(Err(e)) => e,
        Err(_) => EngineError::new(ErrorCode::Internal, "Внутренняя ошибка при обработке файла"),
    };
    emit(&Event::Error {
        code: err.code,
        message: &err.message,
        codec: err.codec.as_deref(),
    });
    ExitCode::from(err.code.exit_code() as u8)
}
