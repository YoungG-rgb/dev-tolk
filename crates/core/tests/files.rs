use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use tolk_core::files::JobFiles;
use tolk_core::jobs::{self, NewJob};
use tolk_core::layout::DataLayout;
use tolk_core::store::Store;
use tolk_core::StoreError;

const JOB: &str = "11111111-2222-3333-4444-555555555555";

fn files_in(dir: &Path) -> JobFiles {
    let layout = DataLayout::new(dir.join("data"));
    layout.ensure().unwrap();
    JobFiles::new(layout)
}

fn ls(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn copy_audio_copies_and_never_touches_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());
    let source = dir.path().join("Записи 1.mp3");
    let payload: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(&source, &payload).unwrap();
    let before = fs::metadata(&source).unwrap().modified().unwrap();

    let bytes = files.copy_audio(JOB, &source, "source.mp3").unwrap();
    assert_eq!(bytes, payload.len() as u64);
    assert_eq!(files.read(JOB, "source.mp3").unwrap().unwrap(), payload);
    assert_eq!(fs::read(&source).unwrap(), payload, "исходник не изменён");
    assert_eq!(fs::metadata(&source).unwrap().modified().unwrap(), before);
    assert!(
        ls(&files.layout().tmp_dir()).is_empty(),
        "временных файлов не осталось"
    );
    assert_eq!(files.job_size(JOB).unwrap(), payload.len() as u64);
}

#[test]
fn copy_of_missing_source_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());
    assert!(files
        .copy_audio(JOB, &dir.path().join("нет.mp3"), "source.mp3")
        .is_err());
    assert!(files.read(JOB, "source.mp3").unwrap().is_none());
    assert!(ls(&files.layout().tmp_dir()).is_empty());
}

#[test]
fn write_atomic_replaces_whole_file() {
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());
    files
        .write_atomic(JOB, "transcript.json", br#"{"v":1}"#)
        .unwrap();
    files
        .write_atomic(JOB, "transcript.json", br#"{"v":2}"#)
        .unwrap();
    assert_eq!(
        files.read(JOB, "transcript.json").unwrap().unwrap(),
        br#"{"v":2}"#
    );
    assert_eq!(ls(&files.layout().job_dir(JOB)), vec!["transcript.json"]);
    assert!(ls(&files.layout().tmp_dir()).is_empty());
}

#[test]
fn ids_and_names_cannot_escape_the_jobs_directory() {
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());
    for bad_id in ["", "..", "../x", "a/b", "a\\b", "C:\\x", "x\0y"] {
        assert!(
            matches!(
                files.write_atomic(bad_id, "t.json", b"x"),
                Err(StoreError::InvalidId(_))
            ),
            "{bad_id:?}"
        );
        assert!(
            matches!(files.delete_job_dir(bad_id), Err(StoreError::InvalidId(_))),
            "{bad_id:?}"
        );
    }
    for bad_name in [
        "",
        ".",
        "..",
        "../t.json",
        "a/b.json",
        "a\\b.json",
        "c:t.json",
    ] {
        assert!(
            matches!(
                files.write_atomic(JOB, bad_name, b"x"),
                Err(StoreError::InvalidId(_))
            ),
            "{bad_name:?}"
        );
    }
    assert!(
        ls(dir.path()).iter().all(|n| n == "data"),
        "за пределами каталога данных ничего не создано"
    );
}

#[test]
fn delete_job_dir_removes_everything_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());
    files.write_atomic(JOB, "original.json", b"{}").unwrap();
    files.write_atomic(JOB, "transcript.json", b"{}").unwrap();
    files.delete_job_dir(JOB).unwrap();
    assert!(!files.layout().job_dir(JOB).exists());
    files.delete_job_dir(JOB).unwrap();
}

#[test]
fn cleanup_removes_tmp_leftovers_and_orphan_dirs_only() {
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());
    let layout = files.layout().clone();
    fs::write(layout.tmp_dir().join("w-abc.part"), b"half").unwrap();
    fs::create_dir(layout.tmp_dir().join("sub")).unwrap();
    files.write_atomic("keep-1", "t.json", b"{}").unwrap();
    files.write_atomic("orphan-1", "t.json", b"{}").unwrap();
    files.write_atomic("orphan-2", "t.json", b"{}").unwrap();
    fs::write(layout.jobs_dir().join("stray.txt"), b"x").unwrap();

    assert_eq!(files.cleanup_tmp().unwrap(), 2);
    assert!(ls(&layout.tmp_dir()).is_empty());
    let known: HashSet<String> = ["keep-1".to_string()].into();
    assert_eq!(
        files.cleanup_orphans(&known).unwrap(),
        vec!["orphan-1", "orphan-2"]
    );
    assert_eq!(ls(&layout.jobs_dir()), vec!["keep-1", "stray.txt"]);
}

// ---- принудительное завершение процесса -----------------------------------------------------------------

/// Рабочая часть теста: выполняется в дочернем процессе (тот же тестовый бинарник), только если задана переменная.
/// Доходит до точки «данные записаны, переименование не выполнено», сообщает об этом файлом-маркером и ждёт.
#[test]
fn kill_child_worker() {
    let Ok(root) = std::env::var("TOLK_KILL_ROOT") else {
        return;
    };
    let mode = std::env::var("TOLK_KILL_MODE").unwrap();
    let root = PathBuf::from(root);
    let files = JobFiles::new(DataLayout::new(root.join("data")));
    let marker = root.join("ready");
    let hook = || {
        fs::write(&marker, b"1").unwrap();
        sleep(Duration::from_secs(120));
    };
    match mode.as_str() {
        "write" => files
            .write_atomic_with_hook(JOB, "transcript.json", &vec![b'N'; 5_000_000], hook)
            .unwrap(),
        "copy" => {
            files
                .copy_audio_with_hook(JOB, &root.join("source-original.mp3"), "source.mp3", hook)
                .unwrap();
        }
        other => panic!("неизвестный режим {other}"),
    }
}

/// Запускает рабочую часть, ждёт точки прерывания и убивает процесс сигналом KILL.
fn run_and_kill(root: &Path, mode: &str) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "kill_child_worker",
            "--test-threads",
            "1",
            "--nocapture",
        ])
        .env("TOLK_KILL_ROOT", root)
        .env("TOLK_KILL_MODE", mode)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !root.join("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "дочерний процесс не дошёл до точки прерывания"
        );
        sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn killed_during_write_keeps_old_version_and_leaves_only_tmp_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());
    files.write_atomic(JOB, "transcript.json", b"OLD").unwrap();

    run_and_kill(dir.path(), "write");

    assert_eq!(
        files.read(JOB, "transcript.json").unwrap().unwrap(),
        b"OLD",
        "прежняя версия цела"
    );
    assert_eq!(ls(&files.layout().job_dir(JOB)), vec!["transcript.json"]);
    assert_eq!(
        ls(&files.layout().tmp_dir()).len(),
        1,
        "остался только временный файл"
    );
    assert_eq!(files.cleanup_tmp().unwrap(), 1);
}

#[test]
fn killed_during_first_write_leaves_no_final_file() {
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());

    run_and_kill(dir.path(), "write");

    assert!(
        files.read(JOB, "transcript.json").unwrap().is_none(),
        "итогового файла нет"
    );
    assert_eq!(files.cleanup_tmp().unwrap(), 1);
}

#[test]
fn killed_during_copy_leaves_no_final_file() {
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());
    fs::write(dir.path().join("source-original.mp3"), vec![7u8; 8_000_000]).unwrap();

    run_and_kill(dir.path(), "copy");

    assert!(
        files.read(JOB, "source.mp3").unwrap().is_none(),
        "итоговой копии нет"
    );
    assert_eq!(
        fs::read(dir.path().join("source-original.mp3"))
            .unwrap()
            .len(),
        8_000_000,
        "исходник цел"
    );
    assert_eq!(files.cleanup_tmp().unwrap(), 1);
}

// ---- вместе с базой ---------------------------------------------------------------------------------------

fn new_job() -> NewJob {
    NewJob {
        source_name: "a.mp3".into(),
        ..NewJob::default()
    }
}

#[test]
fn delete_job_removes_row_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(DataLayout::new(dir.path().join("data"))).unwrap();
    let job = jobs::insert(&store.db.conn(), &new_job()).unwrap();
    store
        .files
        .write_atomic(&job.id, "transcript.json", b"{}")
        .unwrap();

    assert!(store.delete_job(&job.id).unwrap());
    assert!(jobs::get(&store.db.conn(), &job.id).unwrap().is_none());
    assert!(!store.files.layout().job_dir(&job.id).exists());
    assert!(
        !store.delete_job(&job.id).unwrap(),
        "повторное удаление не падает"
    );
}

#[test]
fn startup_removes_dirs_left_by_interrupted_delete_or_import() {
    let dir = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(dir.path().join("data"));
    let (store, first) = Store::open(layout.clone()).unwrap();
    assert_eq!(first, Default::default());
    let kept = jobs::insert(&store.db.conn(), &new_job()).unwrap();
    let gone = jobs::insert(&store.db.conn(), &new_job()).unwrap();
    store.files.write_atomic(&kept.id, "t.json", b"{}").unwrap();
    store.files.write_atomic(&gone.id, "t.json", b"{}").unwrap();
    // Сбой после удаления записи, до удаления каталога.
    jobs::delete(&store.db.conn(), &gone.id).unwrap();
    // Импорт, прерванный до создания записи.
    store
        .files
        .write_atomic("00000000-0000-0000-0000-000000000000", "source.mp3", b"x")
        .unwrap();
    fs::write(layout.tmp_dir().join("w-x.part"), b"x").unwrap();
    drop(store);

    let (store, cleanup) = Store::open(layout).unwrap();
    assert_eq!(cleanup.tmp_removed, 1);
    assert_eq!(cleanup.orphans_removed.len(), 2);
    assert!(
        store.files.layout().job_dir(&kept.id).exists(),
        "каталог задания с записью сохранён"
    );
}

#[cfg(windows)]
#[test]
fn file_locked_by_another_handle_gives_clear_busy_error() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let files = files_in(dir.path());
    files.write_atomic(JOB, "transcript.json", b"OLD").unwrap();
    // Открыт без совместного доступа, как файл в Word или под антивирусом.
    let _lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(files.layout().job_dir(JOB).join("transcript.json"))
        .unwrap();

    let err = files
        .write_atomic(JOB, "transcript.json", b"NEW")
        .unwrap_err();
    assert!(matches!(err, StoreError::FileBusy(_)), "{err:?}");
}

#[test]
fn ensure_space_refuses_absurd_sizes_and_accepts_small_ones() {
    let dir = tempfile::tempdir().unwrap();
    assert!(tolk_core::disk::ensure_space(dir.path(), 1024).is_ok());
    assert!(matches!(
        tolk_core::disk::ensure_space(dir.path(), u64::MAX / 2),
        Err(StoreError::NoSpace)
    ));
    assert!(
        tolk_core::disk::ensure_space(&dir.path().join("нет каталога"), u64::MAX / 2).is_ok(),
        "не удалось узнать — не блокируем"
    );
}
