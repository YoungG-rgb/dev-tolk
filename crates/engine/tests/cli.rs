//! Интеграционные тесты `tolk-engine probe` и `extract` на коротких фикстурах.
//! Фикстуры (тон 440 Гц, 2 с) создаёт `tools/gen-fixtures.sh`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn engine(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tolk-engine"))
        .args(args)
        .output()
        .expect("запуск движка")
}

/// Последняя строка JSON из stdout.
fn last_event(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .expect("пустой stdout");
    serde_json::from_str(line).unwrap_or_else(|e| panic!("не JSON: {line}: {e}"))
}

struct Wav {
    rate: u32,
    channels: u16,
    bits: u16,
    samples: Vec<i16>,
}

fn read_wav(path: &Path) -> Wav {
    let d = fs::read(path).expect("wav прочитан");
    assert_eq!(&d[0..4], b"RIFF");
    assert_eq!(&d[8..12], b"WAVE");
    assert_eq!(&d[36..40], b"data");
    let data_len = u32::from_le_bytes(d[40..44].try_into().unwrap()) as usize;
    assert_eq!(
        d.len(),
        44 + data_len,
        "размер данных в заголовке совпадает с файлом"
    );
    assert_eq!(
        u32::from_le_bytes(d[4..8].try_into().unwrap()) as usize,
        36 + data_len
    );
    Wav {
        channels: u16::from_le_bytes(d[22..24].try_into().unwrap()),
        rate: u32::from_le_bytes(d[24..28].try_into().unwrap()),
        bits: u16::from_le_bytes(d[34..36].try_into().unwrap()),
        samples: d[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect(),
    }
}

fn dominant_freq(w: &Wav) -> f64 {
    let crossings = w
        .samples
        .windows(2)
        .filter(|p| p[0] < 0 && p[1] >= 0)
        .count();
    crossings as f64 / (w.samples.len() as f64 / f64::from(w.rate))
}

const SUPPORTED: &[(&str, &str, &str, bool)] = &[
    ("mp3_44k.mp3", "mp3", "MP3", false),
    ("aac_lc.m4a", "isomp4", "AAC", false),
    ("alac_stereo.m4a", "isomp4", "ALAC", false),
    ("wav_44k.wav", "wave", "PCM", false),
    ("wav_16k.wav", "wave", "PCM", false),
    ("wav_8k.wav", "wave", "PCM", false),
    ("flac.flac", "flac", "FLAC", false),
    ("vorbis.ogg", "ogg", "Vorbis", false),
    ("opus.ogg", "ogg", "Opus", false),
    ("opus.webm", "matroska", "Opus", false),
    ("pcm.mov", "isomp4", "PCM", false),
    ("video_aac.mp4", "isomp4", "AAC", true),
    ("video_aac.mov", "isomp4", "AAC", true),
];

#[test]
fn probe_reports_container_codec_and_duration() {
    for (name, container, codec, has_video) in SUPPORTED {
        let out = engine(&[Path::new("probe"), &fixture(name)]);
        assert!(
            out.status.success(),
            "{name}: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        let ev = last_event(&out);
        assert_eq!(ev["type"], "probe", "{name}");
        assert_eq!(ev["container"], *container, "{name}");
        assert_eq!(ev["codec"], *codec, "{name}");
        assert_eq!(ev["has_video"], *has_video, "{name}");
        let ms = ev["duration_ms"]
            .as_u64()
            .unwrap_or_else(|| panic!("{name}: нет длительности"));
        assert!((1950..=2100).contains(&ms), "{name}: длительность {ms} мс");
    }
}

#[test]
fn extract_produces_16k_mono_wav_with_correct_length_and_pitch() {
    let dir = tempfile::tempdir().unwrap();
    for (name, ..) in SUPPORTED {
        let dest = dir.path().join(format!("{name}.wav"));
        let out = engine(&[Path::new("extract"), &fixture(name), &dest]);
        assert!(
            out.status.success(),
            "{name}: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        let ev = last_event(&out);
        assert_eq!(ev["type"], "done", "{name}");
        assert_eq!(ev["incomplete"], false, "{name}");

        let wav = read_wav(&dest);
        assert_eq!(
            (wav.rate, wav.channels, wav.bits),
            (16_000, 1, 16),
            "{name}"
        );
        let secs = wav.samples.len() as f64 / 16_000.0;
        // AAC добавляет задержку кодера (десятки мс): допуск ±0,1 с, критерий задачи — ±1 с.
        assert!((secs - 2.0).abs() < 0.1, "{name}: {secs} с");
        let freq = dominant_freq(&wav);
        assert!((freq - 440.0).abs() < 5.0, "{name}: частота {freq} Гц");
        assert!(
            wav.samples.iter().map(|s| s.unsigned_abs()).max().unwrap() > 1000,
            "{name}: тишина"
        );
    }
}

#[test]
fn extract_reports_progress_monotonically() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.wav");
    let out = engine(&[Path::new("extract"), &fixture("video_aac.mp4"), &dest]);
    let text = String::from_utf8_lossy(&out.stdout);
    let percents: Vec<u64> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["type"] == "progress")
        .map(|v| v["percent"].as_u64().unwrap())
        .collect();
    assert_eq!(percents.first(), Some(&0));
    assert_eq!(percents.last(), Some(&100));
    assert!(percents.windows(2).all(|w| w[0] <= w[1]), "{percents:?}");
}

#[test]
fn unsupported_codec_is_reported_with_its_name() {
    for name in ["ac3.mp4", "video_ac3.mp4"] {
        let out = engine(&[Path::new("probe"), &fixture(name)]);
        assert_eq!(out.status.code(), Some(10), "{name}");
        let ev = last_event(&out);
        assert_eq!(ev["type"], "error");
        assert_eq!(ev["code"], "UNSUPPORTED_CODEC", "{name}");
        assert_eq!(ev["codec"], "AC-3", "{name}");
    }
}

#[test]
fn video_without_audio_track_is_rejected() {
    let out = engine(&[Path::new("probe"), &fixture("video_noaudio.mp4")]);
    assert_eq!(out.status.code(), Some(10));
    assert_eq!(last_event(&out)["code"], "NO_AUDIO_TRACK");
}

#[test]
fn heaac_core_layer_decodes_with_correct_duration() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("he.wav");
    let out = engine(&[Path::new("extract"), &fixture("heaac.m4a"), &dest]);
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let secs = read_wav(&dest).samples.len() as f64 / 16_000.0;
    assert!((secs - 2.0).abs() < 0.2, "{secs} с");
}

fn broken_inputs(dir: &Path) -> Vec<(&'static str, PathBuf, &'static str)> {
    let mp3 = fs::read(fixture("mp3_44k.mp3")).unwrap();
    let mp4 = fs::read(fixture("video_aac.mp4")).unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    };
    vec![
        ("пустой файл", write("empty.mp3", b""), "CORRUPT_FILE"),
        (
            "текст под видом mp3",
            write("text.mp3", "это не аудио, просто текст".as_bytes()),
            "UNSUPPORTED_FORMAT",
        ),
        (
            "оборванный mp4 без moov",
            write("cut.mp4", &mp4[..mp4.len() / 3]),
            "CORRUPT_FILE",
        ),
        (
            "мусор под видом m4a",
            write("garbage.m4a", &vec![0xA5u8; 4096]),
            "UNSUPPORTED_FORMAT",
        ),
        ("нет файла", dir.join("missing.mp3"), "FILE_NOT_FOUND"),
        ("каталог вместо файла", dir.to_path_buf(), "CORRUPT_FILE"),
    ]
    .into_iter()
    .chain(std::iter::once((
        "mp3 обрезан",
        write("cut.mp3", &mp3[..mp3.len() / 3]),
        "",
    )))
    .collect()
}

#[test]
fn broken_files_give_clear_error_codes_without_panic() {
    let dir = tempfile::tempdir().unwrap();
    for (what, path, code) in broken_inputs(dir.path()) {
        if code.is_empty() {
            continue; // обрезанный mp3 — отдельный тест
        }
        for cmd in ["probe", "extract"] {
            let dest = dir.path().join("never.wav");
            let args: Vec<&Path> = if cmd == "probe" {
                vec![Path::new("probe"), &path]
            } else {
                vec![Path::new("extract"), &path, &dest]
            };
            let out = engine(&args);
            assert_eq!(out.status.code(), Some(10), "{what}/{cmd}");
            let ev = last_event(&out);
            assert_eq!(ev["type"], "error", "{what}/{cmd}");
            assert_eq!(ev["code"], code, "{what}/{cmd}");
            assert!(ev["message"].as_str().is_some_and(|m| !m.is_empty()));
            assert!(
                !String::from_utf8_lossy(&out.stderr).contains("panicked"),
                "{what}/{cmd}: panic"
            );
            assert!(
                !dest.exists(),
                "{what}: итоговый файл не должен появиться при ошибке"
            );
        }
    }
}

#[test]
fn truncated_mp3_yields_partial_audio_flagged_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let mp3 = fs::read(fixture("mp3_44k.mp3")).unwrap();
    let cut = dir.path().join("cut.mp3");
    fs::write(&cut, &mp3[..mp3.len() / 3]).unwrap();
    let dest = dir.path().join("out.wav");
    let out = engine(&[Path::new("extract"), &cut, &dest]);
    assert!(out.status.success());
    let ev = last_event(&out);
    assert_eq!(ev["incomplete"], true);
    let secs = read_wav(&dest).samples.len() as f64 / 16_000.0;
    assert!(secs > 0.3 && secs < 1.5, "{secs} с");
}

#[test]
fn failed_extract_leaves_no_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().join("text.mp3");
    fs::write(&text, b"not audio").unwrap();
    let dest = dir.path().join("out.wav");
    let out = engine(&[Path::new("extract"), &text, &dest]);
    assert_eq!(out.status.code(), Some(10));
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, vec![std::ffi::OsString::from("text.mp3")]);
}

#[test]
fn cyrillic_and_spaces_in_paths_work() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("Иван Петров").join("Записи");
    fs::create_dir_all(&nested).unwrap();
    let src = nested.join("совещание 1.mp3");
    fs::copy(fixture("mp3_44k.mp3"), &src).unwrap();
    let dest = nested.join("результат 1.wav");

    let out = engine(&[Path::new("probe"), &src]);
    assert!(out.status.success());
    let out = engine(&[Path::new("extract"), &src, &dest]);
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(read_wav(&dest).rate, 16_000);
}

#[test]
fn extract_refuses_to_overwrite_its_own_input() {
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("a.mp3");
    fs::copy(fixture("mp3_44k.mp3"), &copy).unwrap();
    let before = fs::read(&copy).unwrap();
    let out = engine(&[Path::new("extract"), &copy, &copy]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(fs::read(&copy).unwrap(), before, "исходный файл не изменён");
}

#[test]
fn usage_errors_exit_with_code_2() {
    for args in [
        vec![],
        vec![Path::new("probe")],
        vec![Path::new("transcribe")],
    ] {
        let out = engine(&args);
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(last_event(&out)["code"], "USAGE");
    }
}
