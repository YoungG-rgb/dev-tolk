//! Тесты `tolk-engine transcribe`. Тесты с настоящей моделью запускаются на macOS, если рядом лежат
//! `ggml-small-q5_1.bin` и `ggml-silero-v6.2.0.bin` (каталог — `$TOLK_TEST_MODELS` или `../dev-tolk-tools/models`),
//! и пропускаются с пометкой, если моделей нет. Речь для них синтезирует `say` (macOS).

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use serde_json::Value;
use tolk_core::transcript::Transcript;

const ENGINE: &str = env!("CARGO_BIN_EXE_tolk-engine");
const PHRASE: &str = "Добрый день, коллеги. Сегодня мы обсудим план работ на следующий квартал. Первым пунктом стоит перенос сервисов в облако. Вторым пунктом, настройка мониторинга и оповещений. Ответственным за миграцию назначен Иван Петров. Срок выполнения, пятнадцатое ноября. Если возникнут вопросы, пишите в общий чат.";

fn models_dir() -> PathBuf {
    std::env::var_os("TOLK_TEST_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../dev-tolk-tools/models")
        })
}

/// Модели для тестов или `None` (тест пропускается).
fn models() -> Option<(PathBuf, PathBuf)> {
    let dir = models_dir();
    let (m, v) = (
        dir.join("ggml-small-q5_1.bin"),
        dir.join("ggml-silero-v6.2.0.bin"),
    );
    (m.is_file() && v.is_file()).then_some((m, v))
}

macro_rules! require_models {
    () => {
        match models() {
            Some(m) => m,
            None => {
                eprintln!(
                    "ПРОПУЩЕН: нет моделей в {} (small-q5_1 и silero-vad)",
                    models_dir().display()
                );
                return;
            }
        }
    };
}

/// Синтезированная русская речь (`copies` повторов фразы). Только macOS.
fn speech(copies: usize) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") || !Path::new("/usr/bin/say").exists() {
        return None;
    }
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<usize, PathBuf>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().unwrap();
    if let Some(p) = cache.get(&copies) {
        return Some(p.clone());
    }
    let dir = std::env::temp_dir().join(format!("tolk-engine-test-speech-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let text = dir.join("text.txt");
    fs::write(&text, vec![PHRASE; copies].join("\n\n")).unwrap();
    let aiff = dir.join("s.aiff");
    let m4a = dir.join(format!("speech{copies}.m4a"));
    let ok = Command::new("/usr/bin/say")
        .args(["-v", "Milena", "-o"])
        .arg(&aiff)
        .arg("-f")
        .arg(&text)
        .status()
        .ok()?
        .success()
        && Command::new("/usr/bin/afconvert")
            .args(["-f", "m4af", "-d", "aac", "-b", "64000"])
            .arg(&aiff)
            .arg(&m4a)
            .status()
            .ok()?
            .success();
    ok.then(|| {
        cache.insert(copies, m4a.clone());
        m4a
    })
}

macro_rules! require_speech {
    ($n:expr) => {
        match speech($n) {
            Some(p) => p,
            None => {
                eprintln!("ПРОПУЩЕН: нет синтезатора речи say");
                return;
            }
        }
    };
}

struct Run {
    code: Option<i32>,
    events: Vec<Value>,
}

impl Run {
    fn last(&self) -> &Value {
        self.events.last().expect("нет событий")
    }
    fn of_type<'a>(&'a self, t: &'a str) -> impl Iterator<Item = &'a Value> {
        self.events.iter().filter(move |e| e["type"] == t)
    }
}

fn transcribe(args: &[&std::ffi::OsStr]) -> Run {
    let out = Command::new(ENGINE)
        .arg("transcribe")
        .args(args)
        .output()
        .expect("запуск");
    let events = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("не JSON: {l}: {e}")))
        .collect();
    Run {
        code: out.status.code(),
        events,
    }
}

fn os(s: &str) -> &std::ffi::OsStr {
    std::ffi::OsStr::new(s)
}

fn leftovers(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".part"))
        .collect()
}

// ---- без модели: аргументы и проверки до дорогой работы -------------------------------------------------

#[test]
fn bad_arguments_exit_with_usage_code() {
    for args in [
        vec![],
        vec![os("--model"), os("m.bin")],
        vec![os("--bogus")],
        vec![os("--model")],
        vec![
            os("--model"),
            os("m"),
            os("--input"),
            os("i"),
            os("--output"),
            os("o"),
            os("--threads"),
            os("abc"),
        ],
    ] {
        let r = transcribe(&args);
        assert_eq!(r.code, Some(2), "{args:?}");
        assert_eq!(r.last()["code"], "USAGE");
    }
}

#[test]
fn missing_or_empty_model_fails_before_decoding() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("o.json");
    let audio = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mp3_44k.mp3");

    let started = std::time::Instant::now();
    let r = transcribe(&[
        os("--model"),
        dir.path().join("нет.bin").as_os_str(),
        os("--input"),
        audio.as_os_str(),
        os("--output"),
        out.as_os_str(),
    ]);
    assert_eq!(
        (r.code, r.last()["code"].as_str()),
        (Some(12), Some("MODEL_MISSING"))
    );
    assert!(
        r.of_type("progress").next().is_none(),
        "декодирование не начиналось"
    );
    assert!(started.elapsed().as_secs() < 2);
    assert!(!out.exists());

    let empty = dir.path().join("empty.bin");
    fs::write(&empty, b"").unwrap();
    let r = transcribe(&[
        os("--model"),
        empty.as_os_str(),
        os("--input"),
        audio.as_os_str(),
        os("--output"),
        out.as_os_str(),
    ]);
    assert_eq!(r.last()["code"], "MODEL_MISSING");
}

#[test]
fn corrupt_model_is_reported_as_invalid_without_panic() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("ggml-garbage.bin");
    fs::write(&model, vec![0xABu8; 4096]).unwrap();
    let out = dir.path().join("o.json");
    let audio = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mp3_44k.mp3");
    let r = transcribe(&[
        os("--model"),
        model.as_os_str(),
        os("--input"),
        audio.as_os_str(),
        os("--output"),
        out.as_os_str(),
    ]);
    assert_eq!(
        (r.code, r.last()["code"].as_str()),
        (Some(12), Some("MODEL_INVALID")),
        "{:?}",
        r.last()
    );
    assert!(!out.exists() && leftovers(dir.path()).is_empty());
}

#[test]
fn bad_input_gives_input_error_and_no_output() {
    let dir = tempfile::tempdir().unwrap();
    let model = dir.path().join("ggml-x.bin");
    fs::write(&model, b"not a model").unwrap();
    let text = dir.path().join("text.mp3");
    fs::write(&text, "это не аудио").unwrap();
    let out = dir.path().join("o.json");
    let r = transcribe(&[
        os("--model"),
        model.as_os_str(),
        os("--input"),
        text.as_os_str(),
        os("--output"),
        out.as_os_str(),
    ]);
    assert_eq!(
        (r.code, r.last()["code"].as_str()),
        (Some(10), Some("UNSUPPORTED_FORMAT"))
    );
    assert!(!out.exists());
}

#[test]
fn output_equal_to_input_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.mp3");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mp3_44k.mp3"),
        &f,
    )
    .unwrap();
    let before = fs::read(&f).unwrap();
    let r = transcribe(&[
        os("--model"),
        f.as_os_str(),
        os("--input"),
        f.as_os_str(),
        os("--output"),
        f.as_os_str(),
    ]);
    assert_eq!(r.code, Some(2));
    assert_eq!(fs::read(&f).unwrap(), before);
}

// ---- с настоящей моделью --------------------------------------------------------------------------------

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('ё', "е")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

#[test]
fn transcribes_synthetic_russian_speech_with_correct_protocol_and_file() {
    let (model, vad) = require_models!();
    let audio = require_speech!(1);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("результат.json");
    let r = transcribe(&[
        os("--model"),
        model.as_os_str(),
        os("--vad-model"),
        vad.as_os_str(),
        os("--input"),
        audio.as_os_str(),
        os("--output"),
        out.as_os_str(),
    ]);
    assert_eq!(r.code, Some(0), "{:?}", r.last());

    // Файл: формат ядра, модель определена по имени файла, сегменты упорядочены.
    let t = Transcript::from_json(&fs::read(&out).unwrap())
        .expect("original.json соответствует формату ядра");
    assert_eq!(
        (t.model.as_str(), t.language.as_str()),
        ("small-q5_1", "ru")
    );
    assert!(t.engine.starts_with("whisper.cpp"));
    assert!(!t.segments.is_empty());
    assert!(t
        .segments
        .windows(2)
        .all(|w| w[0].start_ms <= w[1].start_ms));
    assert!(
        (15_000..30_000).contains(&t.duration_ms),
        "{}",
        t.duration_ms
    );
    assert!(leftovers(dir.path()).is_empty());

    // Качество на чистой синтетической речи: не метрика, а проверка, что конвейер не ломает текст.
    let got = words(
        &t.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    );
    let want = words(PHRASE);
    let missing = want.iter().filter(|w| !got.contains(w)).count();
    assert!(
        missing * 5 <= want.len(),
        "потеряно {missing} из {} слов: {got:?}",
        want.len()
    );

    // Протокол: прогресс 0..100 не убывает, 100 печатается после записи файла, сегменты совпадают с файлом.
    let percents: Vec<u64> = r
        .of_type("progress")
        .map(|e| e["percent"].as_u64().unwrap())
        .collect();
    assert_eq!((percents.first(), percents.last()), (Some(&0), Some(&100)));
    assert!(percents.windows(2).all(|w| w[0] <= w[1]), "{percents:?}");
    let live: Vec<String> = r
        .of_type("segment")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        live,
        t.segments
            .iter()
            .map(|s| s.text.clone())
            .collect::<Vec<_>>()
    );
    let done = r.last();
    assert_eq!(done["type"], "done");
    assert_eq!(
        done["segments"].as_u64().unwrap() as usize,
        t.segments.len()
    );
    assert_eq!(done["audio_ms"].as_u64().unwrap(), t.duration_ms);
    for k in ["elapsed_ms", "model_load_ms", "decode_ms", "recognize_ms"] {
        assert!(done[k].is_u64(), "{k}");
    }
    assert!(done["elapsed_ms"].as_u64().unwrap() >= done["recognize_ms"].as_u64().unwrap());
    if cfg!(target_os = "macos") {
        assert_eq!(done["backend"], "metal");
        assert!(
            done["peak_rss_bytes"].as_u64().unwrap() > 100_000_000,
            "пиковая память не измерена: {done}"
        );
    }
}

#[test]
fn no_gpu_flag_selects_cpu_backend_and_gives_the_same_words() {
    let (model, vad) = require_models!();
    let audio = require_speech!(1);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cpu.json");
    let r = transcribe(&[
        os("--model"),
        model.as_os_str(),
        os("--vad-model"),
        vad.as_os_str(),
        os("--input"),
        audio.as_os_str(),
        os("--output"),
        out.as_os_str(),
        os("--no-gpu"),
        os("--threads"),
        os("4"),
    ]);
    assert_eq!(r.code, Some(0), "{:?}", r.last());
    assert_eq!(r.last()["backend"], "cpu");
    let t = Transcript::from_json(&fs::read(&out).unwrap()).unwrap();
    let got = words(
        &t.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    );
    assert!(
        got.contains(&"квартал".to_string()) && got.contains(&"мониторинга".to_string()),
        "{got:?}"
    );
}

#[cfg(unix)]
#[test]
fn cyrillic_and_space_paths_work_for_model_input_and_output() {
    use std::os::unix::fs::symlink;
    let (model, vad) = require_models!();
    let audio = require_speech!(1);
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("Иван Петров").join("Записи и модели");
    fs::create_dir_all(&base).unwrap();
    let (m, v, a) = (
        base.join("ggml-small-q5_1.bin"),
        base.join("тишина vad.bin"),
        base.join("совещание 1.m4a"),
    );
    symlink(&model, &m).unwrap();
    symlink(&vad, &v).unwrap();
    fs::copy(&audio, &a).unwrap();
    let out = base.join("итог.json");
    let r = transcribe(&[
        os("--model"),
        m.as_os_str(),
        os("--vad-model"),
        v.as_os_str(),
        os("--input"),
        a.as_os_str(),
        os("--output"),
        out.as_os_str(),
    ]);
    assert_eq!(r.code, Some(0), "{:?}", r.last());
    assert!(out.is_file());
}

/// Запускает распознавание длинной записи и возвращает процесс после первого сегмента.
#[cfg(unix)]
#[allow(clippy::zombie_processes)] // вызывающий тест всегда дожидается процесса (wait)
fn start_until_first_segment(
    model: &Path,
    vad: &Path,
    audio: &Path,
    out: &Path,
) -> (
    std::process::Child,
    BufReader<std::process::ChildStdout>,
    Vec<String>,
) {
    let mut child = Command::new(ENGINE)
        .arg("transcribe")
        .args(["--model"])
        .arg(model)
        .args(["--vad-model"])
        .arg(vad)
        .args(["--input"])
        .arg(audio)
        .args(["--output"])
        .arg(out)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "процесс завершился до первого сегмента: {lines:?}"
        );
        let is_segment = line.contains("\"type\":\"segment\"");
        lines.push(line);
        if is_segment {
            return (child, reader, lines);
        }
    }
}

#[cfg(unix)]
#[test]
fn sigterm_cancels_gracefully_without_result_file() {
    let (model, vad) = require_models!();
    let audio = require_speech!(8);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("o.json");
    let (mut child, mut reader, _) = start_until_first_segment(&model, &vad, &audio, &out);
    let asked = std::time::Instant::now();
    // SAFETY: посылаем сигнал собственному дочернему процессу.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let mut rest = String::new();
    std::io::Read::read_to_string(&mut reader, &mut rest).unwrap();
    let status = child.wait().unwrap();
    assert!(
        asked.elapsed().as_secs() < 5,
        "отмена заняла {:?}",
        asked.elapsed()
    );
    assert_eq!(status.code(), Some(130), "{rest}");
    assert!(rest.contains("\"code\":\"CANCELLED\""), "{rest}");
    assert!(!out.exists(), "итогового файла нет");
    assert!(leftovers(dir.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn sigkill_mid_run_leaves_no_result_file() {
    let (model, vad) = require_models!();
    let audio = require_speech!(8);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("o.json");
    let (mut child, _reader, _) = start_until_first_segment(&model, &vad, &audio, &out);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        !out.exists(),
        "после принудительного завершения итогового файла нет"
    );
}

/// Приложение погибло: канал stdin закрылся. Движок не должен остаться висеть и не оставляет результата.
#[cfg(unix)]
#[test]
fn engine_exits_by_itself_when_parent_closes_stdin() {
    let (model, vad) = require_models!();
    let audio = require_speech!(8);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("o.json");
    let mut child = Command::new(ENGINE)
        .arg("transcribe")
        .arg("--model")
        .arg(&model)
        .arg("--vad-model")
        .arg(&vad)
        .arg("--input")
        .arg(&audio)
        .arg("--output")
        .arg(&out)
        .arg("--exit-on-stdin-close")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "процесс завершился раньше времени"
        );
        if line.contains("\"type\":\"segment\"") {
            break;
        }
    }
    drop(child.stdin.take()); // «родитель погиб»
    let started = std::time::Instant::now();
    let status = child.wait().unwrap();
    assert!(
        started.elapsed().as_secs() < 3,
        "движок завершился не сразу: {:?}",
        started.elapsed()
    );
    assert_eq!(status.code(), Some(143));
    assert!(!out.exists(), "результата нет");
    assert!(leftovers(dir.path()).is_empty());
}

#[test]
fn exit_on_stdin_close_flag_is_accepted_without_a_pipe() {
    let r = transcribe(&[os("--exit-on-stdin-close")]);
    assert_eq!(
        r.code,
        Some(2),
        "аргумент известен; ошибка только в отсутствующих обязательных"
    );
    assert!(
        r.last()["message"].as_str().unwrap().contains("--model"),
        "{:?}",
        r.last()
    );
}
