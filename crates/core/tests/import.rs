//! Импорт файлов настоящим `tolk-engine` (probe, extract) и фикстурами движка; отмена — на фиктивном
//! движке. Если собранного движка нет, тесты с ним пропускаются.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tolk_core::download::CancelToken;
use tolk_core::import::{import_file, Imported};
use tolk_core::jobs::{self, JobStatus};
use tolk_core::layout::DataLayout;
use tolk_core::store::Store;
use tolk_core::user_errors::UserMessage;

fn engine() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TOLK_ENGINE") {
        return Some(PathBuf::from(p));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    ["release", "debug"]
        .iter()
        .map(|p| root.join(p).join("tolk-engine"))
        .find(|p| p.is_file())
}

macro_rules! require_engine {
    () => {
        match engine() {
            Some(e) => e,
            None => {
                eprintln!("ПРОПУЩЕН: нет собранного tolk-engine");
                return;
            }
        }
    };
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../engine/tests/fixtures")
        .join(name)
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: Store,
}

fn new_env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (store, _) = Store::open(DataLayout::new(root.join("data"))).unwrap();
    Env {
        _dir: dir,
        root,
        store,
    }
}

impl Env {
    fn import(&self, exe: &Path, src: &Path) -> Result<Imported, UserMessage> {
        import_file(
            &self.store,
            exe,
            src,
            Some("small-q5_1"),
            &CancelToken::new(),
            &mut |_| {},
        )
    }

    fn job_dirs(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.root.join("data").join("jobs"))
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    fn job_count(&self) -> usize {
        jobs::list(&self.store.db.conn()).unwrap().len()
    }
}

#[test]
fn six_mixed_files_are_imported_in_one_go() {
    let exe = require_engine!();
    let env = new_env();
    let files = [
        "mp3_44k.mp3",
        "aac_lc.m4a",
        "opus.ogg",
        "video_aac.mp4",
        "video_aac.mov",
        "wav_16k.wav",
    ];
    let mut videos = 0;
    for name in files {
        let imported = env
            .import(&exe, &fixture(name))
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let job = imported.job;
        assert_eq!(job.status, JobStatus::Queued, "{name}");
        assert_eq!(job.source_name, name);
        assert!(job.duration_ms.unwrap_or(0) > 0, "{name}: длительность");
        assert!(
            imported.warnings.is_empty(),
            "{name}: {:?}",
            imported.warnings
        );
        let audio = job.audio_path.clone().unwrap();
        let path = env.store.files.layout().job_dir(&job.id).join(&audio);
        assert!(path.is_file(), "{name}: аудио в каталоге задания");
        if name.starts_with("video") {
            videos += 1;
            assert_eq!(audio, "source.wav", "видео хранится как извлечённый WAV");
            assert_eq!(&fs::read(&path).unwrap()[..4], b"RIFF");
        } else {
            assert_eq!(
                fs::read(&path).unwrap(),
                fs::read(fixture(name)).unwrap(),
                "{name}: копия байт в байт"
            );
        }
    }
    assert_eq!(videos, 2);
    assert_eq!(env.job_count(), 6);
    assert_eq!(env.job_dirs().len(), 6);
}

#[test]
fn rejected_files_have_clear_reasons_and_leave_nothing_behind() {
    let exe = require_engine!();
    let env = new_env();
    let text = env.root.join("заметка.mp3");
    fs::write(&text, "это не звук, просто текст").unwrap();
    let empty = env.root.join("пустой.mp3");
    fs::write(&empty, b"").unwrap();
    let truncated = env.root.join("обрезанное.mp4");
    fs::write(
        &truncated,
        &fs::read(fixture("video_aac.mp4")).unwrap()[..2000],
    )
    .unwrap();
    let locked = env.root.join("закрытый.mp3");
    fs::copy(fixture("mp3_44k.mp3"), &locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let dir = env.root.join("папка");
    fs::create_dir(&dir).unwrap();

    let cases: Vec<(PathBuf, &str, &str)> = vec![
        (text, "UNSUPPORTED_FORMAT", "формат"),
        (empty, "CORRUPT_FILE", "поврежд"),
        (truncated, "CORRUPT_FILE", "поврежд"),
        (
            fixture("video_noaudio.mp4"),
            "NO_AUDIO_TRACK",
            "аудиодорожки",
        ),
        (fixture("ac3.mp4"), "UNSUPPORTED_CODEC", "AC-3"),
        (fixture("video_ac3.mp4"), "UNSUPPORTED_CODEC", "AC-3"),
        (
            env.root.join("нет такого.mp3"),
            "FILE_NOT_FOUND",
            "не найден",
        ),
        (locked, "ACCESS_DENIED", "доступа"),
        (dir, "UNSUPPORTED_FORMAT", "формат"),
    ];
    for (path, code, fragment) in cases {
        let err = env
            .import(&exe, &path)
            .err()
            .unwrap_or_else(|| panic!("{} принят", path.display()));
        assert_eq!(err.code, code, "{}: {err:?}", path.display());
        assert!(
            err.message
                .to_lowercase()
                .contains(&fragment.to_lowercase()),
            "{}: {}",
            path.display(),
            err.message
        );
        assert!(!err.action.is_empty());
    }
    assert_eq!(env.job_count(), 0);
    assert!(
        env.job_dirs().is_empty(),
        "каталоги заданий отклонённых файлов удалены: {:?}",
        env.job_dirs()
    );
}

#[test]
fn cyrillic_and_spaces_in_paths_work() {
    let exe = require_engine!();
    let env = new_env();
    let dir = env.root.join("Мои записи (июнь)");
    fs::create_dir(&dir).unwrap();
    for (from, to) in [
        ("mp3_44k.mp3", "совещание №1.mp3"),
        ("video_aac.mp4", "видео с телефона.mp4"),
    ] {
        let path = dir.join(to);
        fs::copy(fixture(from), &path).unwrap();
        let imported = env.import(&exe, &path).unwrap();
        assert_eq!(imported.job.source_name, to);
    }
}

#[test]
fn re_adding_a_file_warns_but_is_allowed() {
    let exe = require_engine!();
    let env = new_env();
    let first = env.import(&exe, &fixture("opus.ogg")).unwrap();
    assert!(first.warnings.is_empty());
    let second = env.import(&exe, &fixture("opus.ogg")).unwrap();
    assert_eq!(second.warnings.len(), 1);
    assert_eq!(second.warnings[0].code, "DUPLICATE");
    assert!(second.warnings[0].message.contains("в очереди"));
    assert_ne!(first.job.id, second.job.id);
    assert_eq!(env.job_count(), 2);
}

#[test]
fn import_does_not_modify_the_source_file() {
    let exe = require_engine!();
    let env = new_env();
    let src = env.root.join("исходник.mp4");
    fs::copy(fixture("video_aac.mp4"), &src).unwrap();
    let before = fs::read(&src).unwrap();
    env.import(&exe, &src).unwrap();
    assert_eq!(fs::read(&src).unwrap(), before);
}

#[test]
fn hour_long_video_extraction_reports_progress_without_blocking() {
    let exe = require_engine!();
    // Часовое видео из фикстуры собирается ffmpeg; без него тест пропускается.
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("ПРОПУЩЕН: нет ffmpeg");
        return;
    }
    let env = new_env();
    let src = env.root.join("час.mp4");
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=300:duration=3600:sample_rate=44100",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x64:r=1:d=3600",
            "-c:a",
            "aac",
            "-b:a",
            "48k",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-shortest",
        ])
        .arg(&src)
        .status()
        .unwrap();
    assert!(status.success());
    let mut percents = Vec::new();
    let t0 = Instant::now();
    let imported = import_file(
        &env.store,
        &exe,
        &src,
        None,
        &CancelToken::new(),
        &mut |p| percents.push(p),
    )
    .unwrap();
    assert!(
        imported.job.duration_ms.unwrap() >= 3_599_000,
        "{:?}",
        imported.job.duration_ms
    );
    assert!(
        percents.len() > 3 && percents.windows(2).all(|w| w[0] <= w[1]),
        "{percents:?}"
    );
    assert_eq!(*percents.last().unwrap(), 100);
    eprintln!("Извлечение часового видео: {:?}", t0.elapsed());
}

mod fake_engine {
    use super::*;

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake.sh");
        fs::write(&path, format!("#!/bin/bash\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn cancelling_an_extraction_leaves_no_job_and_no_files() {
        let env = new_env();
        let src = env.root.join("видео.mp4");
        fs::write(&src, b"not really a video").unwrap();
        let exe = script(
            &env.root,
            r#"if [ "$1" = probe ]; then echo '{"type":"probe","container":"isomp4","codec":"AAC","duration_ms":60000,"has_video":true}'; exit 0; fi
for i in $(seq 100); do echo '{"type":"progress","percent":5}'; sleep 0.1; done"#,
        );
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            c2.cancel();
        });
        let t0 = Instant::now();
        let err =
            import_file(&env.store, &exe, &src, None, &cancel, &mut |_| {}).expect_err("отменено");
        canceller.join().unwrap();
        assert_eq!(err.code, "CANCELLED");
        assert!(t0.elapsed() < Duration::from_secs(4), "{:?}", t0.elapsed());
        assert_eq!(env.job_count(), 0);
        assert!(env.job_dirs().is_empty());
    }

    #[test]
    fn engine_crash_during_probe_is_reported_as_internal_error() {
        let env = new_env();
        let src = env.root.join("а.mp3");
        fs::write(&src, b"x").unwrap();
        let exe = script(&env.root, "exit 9");
        let err = import_file(
            &env.store,
            &exe,
            &src,
            None,
            &CancelToken::new(),
            &mut |_| {},
        )
        .err()
        .unwrap();
        assert_eq!(err.code, "INTERNAL");
        assert_eq!(env.job_count(), 0);
    }

    #[test]
    fn incomplete_extraction_is_accepted_with_a_warning() {
        let env = new_env();
        let src = env.root.join("рваное.mp4");
        fs::write(&src, b"x").unwrap();
        let exe = script(
            &env.root,
            r#"if [ "$1" = probe ]; then echo '{"type":"probe","container":"isomp4","codec":"AAC","duration_ms":60000,"has_video":true}'; exit 0; fi
echo '{"type":"progress","percent":100}'
printf RIFF > "$3"
echo '{"type":"done","output":"x","duration_ms":30000,"samples":480000,"incomplete":true,"skipped_packets":3}'"#,
        );
        let imported = import_file(
            &env.store,
            &exe,
            &src,
            None,
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(imported.warnings.len(), 1);
        assert_eq!(imported.warnings[0].code, "INCOMPLETE");
        assert_eq!(imported.job.duration_ms, Some(30000));
    }
}

#[test]
fn batch_import_reports_each_file_and_continues_after_a_rejection() {
    use tolk_core::import::{import_many, ImportEvent};
    let exe = require_engine!();
    let env = new_env();
    let bad = env.root.join("не звук.mp3");
    fs::write(&bad, "текст").unwrap();
    let paths = vec![fixture("mp3_44k.mp3"), bad, fixture("video_aac.mp4")];
    let mut events = Vec::new();
    let mut accepted_jobs = 0;
    import_many(
        &env.store,
        &exe,
        &paths,
        Some("small-q5_1"),
        &CancelToken::new(),
        &mut |e| events.push(e),
        &mut |_| accepted_jobs += 1,
    );
    assert_eq!(accepted_jobs, 2);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            ImportEvent::ImportStarted { .. } => "started",
            ImportEvent::ImportProgress { .. } => "progress",
            ImportEvent::ImportAccepted { .. } => "accepted",
            ImportEvent::ImportRejected { .. } => "rejected",
            ImportEvent::ImportBatchDone { .. } => "done",
            ImportEvent::ImportBusy => "busy",
            ImportEvent::ImportNotice { .. } => "notice",
        })
        .collect();
    assert_eq!(kinds.first(), Some(&"started"));
    assert_eq!(kinds.last(), Some(&"done"));
    assert_eq!(kinds.iter().filter(|k| **k == "accepted").count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == "rejected").count(), 1);
    assert!(
        kinds.contains(&"progress"),
        "извлечение из видео показывает прогресс"
    );
    assert!(matches!(
        events.last(),
        Some(ImportEvent::ImportBatchDone {
            total: 3,
            accepted: 2,
            rejected: 1,
            cancelled: false
        })
    ));
    assert_eq!(env.job_count(), 2);
}

#[test]
fn cancelling_a_batch_stops_before_the_next_file() {
    use tolk_core::import::{import_many, ImportEvent};
    let exe = require_engine!();
    let env = new_env();
    let cancel = CancelToken::new();
    let mut events = Vec::new();
    let paths = vec![
        fixture("mp3_44k.mp3"),
        fixture("opus.ogg"),
        fixture("aac_lc.m4a"),
    ];
    import_many(
        &env.store,
        &exe,
        &paths,
        None,
        &cancel,
        &mut |e| events.push(e),
        &mut |_| cancel.cancel(),
    );
    assert_eq!(
        env.job_count(),
        1,
        "после отмены следующие файлы не берутся"
    );
    assert!(matches!(
        events.last(),
        Some(ImportEvent::ImportBatchDone {
            accepted: 1,
            cancelled: true,
            ..
        })
    ));
}
