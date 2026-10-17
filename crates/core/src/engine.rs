//! Запуск процесса `tolk-engine` и разбор его событий (TD-04, MVP-013).
//!
//! Движок — отдельный процесс: падение, нехватка памяти или зависание декодера не затрагивают приложение.
//! Модуль не зависит от Tauri: события передаются вызывающему коду обратным вызовом.
//!
//! - stdout движка — JSON Lines (`docs/ENGINE_PROTOCOL.md`), разбор терпим к лишним строкам;
//! - stderr пишется в файл журнала, пользователю не показывается и текстов транскриптов не содержит;
//! - отмена: SIGTERM (движок завершается сам), через `KILL_AFTER` принудительное завершение;
//! - stdin остаётся открытым каналом: если приложение погибло, движок видит закрытие и завершается сам
//!   (`--exit-on-stdin-close`), поэтому «осиротевших» процессов не остаётся.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::download::CancelToken;

/// Через сколько после SIGTERM процесс завершается принудительно.
pub const KILL_AFTER: Duration = Duration::from_millis(1500);
/// Журнал stderr усекается при запуске, если больше этого размера.
const STDERR_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 2000;

#[derive(Debug, Clone)]
pub struct TranscribeArgs {
    pub model: PathBuf,
    pub vad_model: Option<PathBuf>,
    pub input: PathBuf,
    pub output: PathBuf,
    pub language: String,
    pub threads: Option<usize>,
    pub no_gpu: bool,
}

impl TranscribeArgs {
    pub fn to_command_args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "transcribe".into(),
            "--model".into(),
            self.model.clone().into(),
            "--input".into(),
            self.input.clone().into(),
            "--output".into(),
            self.output.clone().into(),
            "--language".into(),
            self.language.clone().into(),
            "--exit-on-stdin-close".into(),
        ];
        if let Some(vad) = &self.vad_model {
            args.extend(["--vad-model".into(), vad.clone().into()]);
        }
        if let Some(t) = self.threads {
            args.extend(["--threads".into(), t.to_string().into()]);
        }
        if self.no_gpu {
            args.push("--no-gpu".into());
        }
        args
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DoneInfo {
    pub output: String,
    pub segments: usize,
    pub audio_ms: u64,
    pub elapsed_ms: u64,
    pub model_load_ms: u64,
    pub decode_ms: u64,
    pub recognize_ms: u64,
    #[serde(default)]
    pub peak_rss_bytes: Option<u64>,
    pub backend: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub codec: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    Progress {
        percent: u8,
    },
    Segment {
        start_ms: u64,
        end_ms: u64,
        text: String,
    },
    Done(DoneInfo),
    Error(ErrorInfo),
}

/// Разбирает строку stdout. Пустые и незнакомые строки пропускаются (`None`): новая версия движка
/// не должна ломать старое приложение.
pub fn parse_line(line: &str) -> Option<EngineEvent> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// Чем закончился запуск.
#[derive(Debug, Clone, PartialEq)]
pub enum RunResult {
    Done(DoneInfo),
    /// Движок сообщил об ошибке (код и сообщение из протокола).
    Failed(ErrorInfo),
    /// Отменено пользователем (процесс завершён).
    Cancelled,
    /// Процесс завершился без `done` и без `error`: сбой, нехватка памяти, убит ОС.
    Crashed {
        exit_code: Option<i32>,
        stderr_tail: String,
    },
}

/// Запускает `transcribe` и ждёт завершения. `on_event` вызывается из вызывающего потока.
/// Ошибка `io::Error` — только если процесс не удалось запустить.
pub fn run_transcribe(
    exe: &Path,
    args: &TranscribeArgs,
    stderr_log: Option<&Path>,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(&EngineEvent),
) -> io::Result<RunResult> {
    let mut command = Command::new(exe);
    command
        .args(args.to_command_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    match stderr_log {
        Some(path) => command.stderr(open_log(path)?),
        None => command.stderr(Stdio::null()),
    };
    let mut child = command.spawn()?;
    // Удерживаем stdin открытым до конца запуска: закрытие этого канала — сигнал «родитель погиб».
    let _stdin_keepalive = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout запрошен как piped");

    let (tx, rx) = mpsc::channel::<EngineEvent>();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(event) = parse_line(&line) {
                if tx.send(event).is_err() {
                    break;
                }
            }
        }
    });

    let mut terminal: Option<RunResult> = None;
    let mut cancel_requested_at: Option<Instant> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                match &event {
                    EngineEvent::Done(info) => terminal = Some(RunResult::Done(info.clone())),
                    EngineEvent::Error(info) => terminal = Some(RunResult::Failed(info.clone())),
                    _ => {}
                }
                on_event(&event);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if cancel.is_cancelled() {
            match cancel_requested_at {
                None => {
                    cancel_requested_at = Some(Instant::now());
                    terminate(&mut child);
                }
                Some(at) if at.elapsed() >= KILL_AFTER => {
                    let _ = child.kill();
                }
                Some(_) => {}
            }
        }
    }
    let _ = reader.join();
    let status = child.wait()?;

    if cancel_requested_at.is_some() && !matches!(terminal, Some(RunResult::Done(_))) {
        return Ok(RunResult::Cancelled);
    }
    Ok(terminal.unwrap_or_else(|| RunResult::Crashed {
        exit_code: status.code(),
        stderr_tail: stderr_log.map(tail_of).unwrap_or_default(),
    }))
}

fn open_log(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if fs::metadata(path).is_ok_and(|m| m.len() > STDERR_LOG_MAX_BYTES) {
        fs::write(path, b"")?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn tail_of(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(STDERR_TAIL_BYTES as u64);
    let mut buf = Vec::new();
    if io::Seek::seek(&mut file, io::SeekFrom::Start(start)).is_ok() {
        let _ = file.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Мягкое завершение: SIGTERM на Unix; на Windows процесс завершается сразу.
fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        // SAFETY: сигнал собственному дочернему процессу.
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

/// Куда писать stderr движка: `logs/engine.log` в каталоге данных.
pub fn engine_log_path(layout: &crate::layout::DataLayout) -> PathBuf {
    layout.logs_dir().join("engine.log")
}
