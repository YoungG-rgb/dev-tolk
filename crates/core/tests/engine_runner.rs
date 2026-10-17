use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tolk_core::download::CancelToken;
use tolk_core::engine::{
    parse_line, run_transcribe, EngineEvent, RunResult, TranscribeArgs, KILL_AFTER,
};

fn args(dir: &Path) -> TranscribeArgs {
    TranscribeArgs {
        model: dir.join("ggml-test.bin"),
        vad_model: Some(dir.join("vad.bin")),
        input: dir.join("in.m4a"),
        output: dir.join("out.json"),
        language: "ru".into(),
        threads: Some(4),
        no_gpu: false,
    }
}

#[test]
fn command_line_matches_the_engine_protocol() {
    let d = Path::new("/tmp/x");
    let a = args(d).to_command_args();
    let text: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
    assert_eq!(text[0], "transcribe");
    for flag in [
        "--model",
        "--input",
        "--output",
        "--language",
        "--vad-model",
        "--threads",
        "--exit-on-stdin-close",
    ] {
        assert!(text.contains(&flag.to_string()), "{flag}: {text:?}");
    }
    assert!(!text.contains(&"--no-gpu".to_string()));
    let mut no_vad = args(d);
    no_vad.vad_model = None;
    no_vad.threads = None;
    no_vad.no_gpu = true;
    let text: Vec<String> = no_vad
        .to_command_args()
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert!(
        text.contains(&"--no-gpu".to_string())
            && !text.contains(&"--vad-model".to_string())
            && !text.contains(&"--threads".to_string())
    );
}

#[test]
fn every_event_kind_is_parsed_and_noise_is_ignored() {
    assert_eq!(
        parse_line(r#"{"type":"progress","percent":42}"#),
        Some(EngineEvent::Progress { percent: 42 })
    );
    assert_eq!(
        parse_line(r#"{"type":"segment","start_ms":0,"end_ms":3720,"text":"Привет"}"#),
        Some(EngineEvent::Segment {
            start_ms: 0,
            end_ms: 3720,
            text: "Привет".into()
        })
    );
    let done = parse_line(
        r#"{"type":"done","output":"/o.json","segments":7,"audio_ms":20619,"elapsed_ms":1532,"model_load_ms":160,"decode_ms":12,"recognize_ms":1354,"peak_rss_bytes":500188264,"backend":"metal","future_field":1}"#,
    );
    match done {
        Some(EngineEvent::Done(d)) => assert_eq!(
            (d.segments, d.peak_rss_bytes, d.backend.as_str()),
            (7, Some(500_188_264), "metal")
        ),
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(parse_line(r#"{"type":"done","output":"x","segments":1,"audio_ms":1,"elapsed_ms":1,"model_load_ms":1,"decode_ms":1,"recognize_ms":1,"backend":"cpu"}"#), Some(EngineEvent::Done(d)) if d.peak_rss_bytes.is_none())
    );
    assert_eq!(
        parse_line(r#"{"type":"error","code":"UNSUPPORTED_CODEC","message":"m","codec":"AC-3"}"#),
        Some(EngineEvent::Error(tolk_core::engine::ErrorInfo {
            code: "UNSUPPORTED_CODEC".into(),
            message: "m".into(),
            codec: Some("AC-3".into())
        }))
    );
    for noise in [
        "",
        "   ",
        "not json",
        "{}",
        r#"{"type":"unknown_future_event"}"#,
        "ggml_metal_init: ...",
        r#"{"type":"progress"}"#,
    ] {
        assert_eq!(parse_line(noise), None, "{noise:?}");
    }
}

#[test]
fn nonexistent_engine_is_an_io_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let r = run_transcribe(
        &dir.path().join("нет-движка"),
        &args(dir.path()),
        None,
        &CancelToken::new(),
        &mut |_| {},
    );
    assert!(r.is_err());
}

#[cfg(unix)]
mod fake_engine {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/bash\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const DONE: &str = r#"{"type":"done","output":"/o.json","segments":1,"audio_ms":1000,"elapsed_ms":10,"model_load_ms":1,"decode_ms":1,"recognize_ms":8,"peak_rss_bytes":123456,"backend":"cpu"}"#;

    fn run(
        exe: &Path,
        dir: &Path,
        cancel: &CancelToken,
        log: Option<&Path>,
    ) -> (RunResult, Vec<EngineEvent>) {
        let mut events = Vec::new();
        let result = run_transcribe(exe, &args(dir), log, cancel, &mut |e| {
            events.push(e.clone())
        })
        .unwrap();
        (result, events)
    }

    #[test]
    fn success_delivers_events_in_order_and_ignores_noise() {
        let dir = tempfile::tempdir().unwrap();
        let exe = script(
            dir.path(),
            "ok.sh",
            &format!("echo 'шум в stdout'\necho '{{\"type\":\"progress\",\"percent\":0}}'\necho '{{\"type\":\"segment\",\"start_ms\":0,\"end_ms\":900,\"text\":\"раз\"}}'\necho '{{\"type\":\"future\"}}'\necho '{{\"type\":\"progress\",\"percent\":100}}'\necho '{DONE}'"),
        );
        let (result, events) = run(&exe, dir.path(), &CancelToken::new(), None);
        assert!(
            matches!(&result, RunResult::Done(d) if d.peak_rss_bytes == Some(123_456)),
            "{result:?}"
        );
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], EngineEvent::Progress { percent: 0 }));
        assert!(matches!(&events[1], EngineEvent::Segment { text, .. } if text == "раз"));
        assert!(matches!(events[3], EngineEvent::Done(_)));
    }

    #[test]
    fn error_event_becomes_failed_with_code() {
        let dir = tempfile::tempdir().unwrap();
        let exe = script(
            dir.path(),
            "fail.sh",
            r#"echo '{"type":"error","code":"MODEL_MISSING","message":"нет"}'; exit 12"#,
        );
        let (result, _) = run(&exe, dir.path(), &CancelToken::new(), None);
        assert!(
            matches!(&result, RunResult::Failed(e) if e.code == "MODEL_MISSING"),
            "{result:?}"
        );
    }

    #[test]
    fn crash_without_events_is_reported_with_stderr_tail_for_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("logs").join("engine.log");
        let exe = script(dir.path(), "crash.sh", "echo '{\"type\":\"progress\",\"percent\":3}'\necho 'ggml: out of memory' >&2\nkill -9 $$");
        let (result, events) = run(&exe, dir.path(), &CancelToken::new(), Some(&log));
        assert_eq!(events.len(), 1);
        match result {
            RunResult::Crashed {
                exit_code,
                stderr_tail,
            } => {
                assert_eq!(exit_code, None, "убит сигналом");
                assert!(stderr_tail.contains("out of memory"), "{stderr_tail}");
            }
            other => panic!("{other:?}"),
        }
        assert!(fs::read_to_string(&log).unwrap().contains("out of memory"));

        let exe = script(dir.path(), "exit3.sh", "exit 3");
        let (result, _) = run(&exe, dir.path(), &CancelToken::new(), None);
        assert!(
            matches!(
                result,
                RunResult::Crashed {
                    exit_code: Some(3),
                    ..
                }
            ),
            "{result:?}"
        );
    }

    #[test]
    fn cancel_terminates_the_process_quickly() {
        let dir = tempfile::tempdir().unwrap();
        let exe = script(
            dir.path(),
            "slow.sh",
            "echo '{\"type\":\"progress\",\"percent\":1}'\nexec sleep 30",
        );
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let started = Instant::now();
        let mut asked = None;
        let result = run_transcribe(&exe, &args(dir.path()), None, &cancel, &mut |e| {
            if matches!(e, EngineEvent::Progress { .. }) {
                asked = Some(Instant::now());
                c.cancel();
            }
        })
        .unwrap();
        assert_eq!(result, RunResult::Cancelled);
        assert!(
            asked.unwrap().elapsed() < Duration::from_secs(2),
            "отмена заняла {:?}",
            asked.unwrap().elapsed()
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn process_ignoring_sigterm_is_killed_after_the_grace_period() {
        let dir = tempfile::tempdir().unwrap();
        let exe = script(dir.path(), "stubborn.sh", "trap '' TERM\necho '{\"type\":\"progress\",\"percent\":1}'\nwhile true; do sleep 0.1; done");
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let started = Instant::now();
        let result =
            run_transcribe(&exe, &args(dir.path()), None, &cancel, &mut |_| c.cancel()).unwrap();
        assert_eq!(result, RunResult::Cancelled);
        let took = started.elapsed();
        assert!(
            took >= KILL_AFTER,
            "дали время на мягкое завершение: {took:?}"
        );
        assert!(took < KILL_AFTER + Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn cancel_after_done_does_not_turn_success_into_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let exe = script(dir.path(), "ok.sh", &format!("echo '{DONE}'"));
        let cancel = CancelToken::new();
        let c = cancel.clone();
        let mut result =
            run_transcribe(&exe, &args(dir.path()), None, &cancel, &mut |_| c.cancel()).unwrap();
        // Если отмену заметили раньше завершения процесса, она не должна затирать готовый результат.
        if let RunResult::Cancelled = result {
            panic!("успешный результат потерян");
        }
        assert!(matches!(
            std::mem::replace(&mut result, RunResult::Cancelled),
            RunResult::Done(_)
        ));
    }

    /// Пока идёт запуск, stdin движка открыт: закрытый канал — сигнал «приложение погибло».
    /// (Проверка на Python: `read -t` в bash 3.2 на macOS возвращает 1 и по таймауту, и по EOF.)
    #[test]
    fn stdin_stays_open_while_the_engine_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stdin.py");
        fs::write(
            &path,
            format!(
                "#!/usr/bin/env python3\nimport select, sys, time\ntime.sleep(0.3)\nready, _, _ = select.select([sys.stdin], [], [], 0.7)\nif ready and sys.stdin.buffer.read1(1) == b'':\n    print('{{\"type\":\"error\",\"code\":\"INTERNAL\",\"message\":\"stdin closed\"}}')\n    sys.exit(1)\nprint('{}')\n",
                DONE
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let (result, _) = run(&path, dir.path(), &CancelToken::new(), None);
        assert!(matches!(result, RunResult::Done(_)), "{result:?}");
    }

    #[test]
    fn oversized_stderr_log_is_truncated_on_start() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("engine.log");
        fs::write(&log, vec![b'x'; 6 * 1024 * 1024]).unwrap();
        let exe = script(
            dir.path(),
            "ok.sh",
            &format!("echo 'свежая строка' >&2\necho '{DONE}'"),
        );
        run(&exe, dir.path(), &CancelToken::new(), Some(&log));
        let text = fs::read_to_string(&log).unwrap();
        assert!(
            text.len() < 1024 && text.contains("свежая строка"),
            "{}",
            text.len()
        );
    }
}

// ---- с настоящим движком ----------------------------------------------------------------------------------

fn real_engine() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TOLK_ENGINE") {
        return Some(PathBuf::from(p));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    ["release", "debug"]
        .iter()
        .map(|p| root.join(p).join("tolk-engine"))
        .find(|p| p.is_file())
}

fn models() -> Option<(PathBuf, PathBuf)> {
    let dir = std::env::var_os("TOLK_TEST_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../dev-tolk-tools/models")
        });
    let (m, v) = (
        dir.join("ggml-small-q5_1.bin"),
        dir.join("ggml-silero-v6.2.0.bin"),
    );
    (m.is_file() && v.is_file()).then_some((m, v))
}

#[cfg(target_os = "macos")]
fn speech(copies: usize) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "tolk-core-runner-speech-{}-{copies}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).ok()?;
    let phrase = "Добрый день, коллеги. Сегодня мы обсудим план работ на следующий квартал. Первым пунктом стоит перенос сервисов в облако.";
    let text = dir.join("t.txt");
    fs::write(&text, vec![phrase; copies].join("\n\n")).ok()?;
    let (aiff, m4a) = (dir.join("s.aiff"), dir.join("s.m4a"));
    let ok = std::process::Command::new("/usr/bin/say")
        .args(["-v", "Milena", "-o"])
        .arg(&aiff)
        .arg("-f")
        .arg(&text)
        .status()
        .ok()?
        .success()
        && std::process::Command::new("/usr/bin/afconvert")
            .args(["-f", "m4af", "-d", "aac", "-b", "64000"])
            .arg(&aiff)
            .arg(&m4a)
            .status()
            .ok()?
            .success();
    ok.then_some(m4a)
}

#[cfg(target_os = "macos")]
#[test]
fn real_engine_transcribes_and_reports_events_through_the_runner() {
    let (Some(exe), Some((model, vad))) = (real_engine(), models()) else {
        eprintln!("ПРОПУЩЕН: нет собранного движка или моделей");
        return;
    };
    let Some(audio) = speech(1) else { return };
    let dir = tempfile::tempdir().unwrap();
    let a = TranscribeArgs {
        model,
        vad_model: Some(vad),
        input: audio,
        output: dir.path().join("o.json"),
        language: "ru".into(),
        threads: Some(4),
        no_gpu: false,
    };
    let log = dir.path().join("logs").join("engine.log");
    let mut percents = Vec::new();
    let mut segments = 0;
    let result = run_transcribe(
        &exe,
        &a,
        Some(&log),
        &CancelToken::new(),
        &mut |e| match e {
            EngineEvent::Progress { percent } => percents.push(*percent),
            EngineEvent::Segment { .. } => segments += 1,
            _ => {}
        },
    )
    .unwrap();
    assert!(
        matches!(&result, RunResult::Done(d) if d.segments == segments && d.backend == "metal"),
        "{result:?}"
    );
    assert!(percents.windows(2).all(|w| w[0] <= w[1]) && percents.last() == Some(&100));
    assert!(dir.path().join("o.json").is_file());
    assert!(log.is_file(), "stderr движка записан в журнал");
}

#[cfg(target_os = "macos")]
#[test]
fn real_engine_cancel_finishes_within_two_seconds_and_leaves_no_result() {
    let (Some(exe), Some((model, vad))) = (real_engine(), models()) else {
        eprintln!("ПРОПУЩЕН: нет собранного движка или моделей");
        return;
    };
    let Some(audio) = speech(8) else { return };
    let dir = tempfile::tempdir().unwrap();
    let a = TranscribeArgs {
        model,
        vad_model: Some(vad),
        input: audio,
        output: dir.path().join("o.json"),
        language: "ru".into(),
        threads: Some(4),
        no_gpu: false,
    };
    let cancel = CancelToken::new();
    let c = cancel.clone();
    let mut asked: Option<Instant> = None;
    let result = run_transcribe(&exe, &a, None, &cancel, &mut |e| {
        if asked.is_none() && matches!(e, EngineEvent::Segment { .. }) {
            asked = Some(Instant::now());
            c.cancel();
        }
    })
    .unwrap();
    assert_eq!(result, RunResult::Cancelled);
    let took = asked.expect("отмену запросили").elapsed();
    assert!(took < Duration::from_secs(2), "отмена заняла {took:?}");
    assert!(!dir.path().join("o.json").exists());
}

#[test]
fn real_engine_reports_missing_model_as_failed_event() {
    let Some(exe) = real_engine() else {
        eprintln!("ПРОПУЩЕН: нет собранного движка");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut a = args(dir.path());
    a.input = Path::new(env!("CARGO_MANIFEST_DIR")).join("../engine/tests/fixtures/mp3_44k.mp3");
    let result = run_transcribe(&exe, &a, None, &CancelToken::new(), &mut |_| {}).unwrap();
    assert!(
        matches!(&result, RunResult::Failed(e) if e.code == "MODEL_MISSING"),
        "{result:?}"
    );
}
