//! Удаление заданий: только завершённые, запись и файлы вместе.

use std::fs;

use tolk_core::jobs::{self, NewJob};
use tolk_core::layout::DataLayout;
use tolk_core::queue;
use tolk_core::store::Store;
use tolk_core::StoreError;

fn setup() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(DataLayout::new(dir.path().join("data"))).unwrap();
    (dir, store)
}

fn add(store: &Store, dir: &tempfile::TempDir) -> String {
    let job = {
        let conn = store.db.conn();
        jobs::insert(
            &conn,
            &NewJob {
                source_name: "запись.mp3".into(),
                audio_path: Some("source.mp3".into()),
                ..Default::default()
            },
        )
        .unwrap()
    };
    let src = dir.path().join("src.mp3");
    fs::write(&src, b"audio").unwrap();
    store.files.copy_audio(&job.id, &src, "source.mp3").unwrap();
    store
        .files
        .write_atomic(&job.id, "transcript.json", b"{}")
        .unwrap();
    job.id
}

fn job_dir(store: &Store, id: &str) -> std::path::PathBuf {
    store.files.layout().job_dir(id)
}

#[test]
fn deleting_a_completed_job_removes_record_and_files() {
    let (dir, store) = setup();
    let id = add(&store, &dir);
    {
        let conn = store.db.conn();
        queue::start(&conn, &id).unwrap();
        queue::complete(&conn, &id).unwrap();
    }
    assert!(job_dir(&store, &id).join("source.mp3").is_file());
    store.delete_finished_job(&id).unwrap();
    assert!(
        !job_dir(&store, &id).exists(),
        "каталог задания удалён с диска"
    );
    assert!(jobs::get(&store.db.conn(), &id).unwrap().is_none());
}

#[test]
fn failed_and_cancelled_jobs_can_be_deleted() {
    let (dir, store) = setup();
    let failed = add(&store, &dir);
    let cancelled = add(&store, &dir);
    {
        let conn = store.db.conn();
        queue::start(&conn, &failed).unwrap();
        queue::fail(&conn, &failed, "INTERNAL", "ошибка").unwrap();
        queue::cancel(&conn, &cancelled).unwrap();
    }
    store.delete_finished_job(&failed).unwrap();
    store.delete_finished_job(&cancelled).unwrap();
    assert!(!job_dir(&store, &failed).exists());
    assert!(!job_dir(&store, &cancelled).exists());
}

#[test]
fn queued_and_processing_jobs_are_not_deleted() {
    let (dir, store) = setup();
    let running = add(&store, &dir);
    let queued = add(&store, &dir);
    queue::start(&store.db.conn(), &running).unwrap();
    for id in [&running, &queued] {
        let err = store.delete_finished_job(id).unwrap_err();
        assert!(
            matches!(err, StoreError::InvalidTransition { .. }),
            "{err:?}"
        );
        assert!(
            job_dir(&store, id).join("source.mp3").is_file(),
            "файлы целы"
        );
        assert!(
            jobs::get(&store.db.conn(), id).unwrap().is_some(),
            "запись цела"
        );
    }
}

#[test]
fn deleting_an_unknown_job_is_an_error_and_touches_nothing() {
    let (dir, store) = setup();
    let id = add(&store, &dir);
    let err = store
        .delete_finished_job("00000000-0000-0000-0000-000000000000")
        .unwrap_err();
    assert!(matches!(err, StoreError::JobNotFound(_)));
    assert!(job_dir(&store, &id).is_dir());
}
