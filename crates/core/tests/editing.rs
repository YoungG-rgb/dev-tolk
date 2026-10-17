//! Правка транскрипта: чтение, сохранение, `has_edits`, возврат к оригиналу, защита от порчи.

use std::fs;

use tolk_core::editing::{load, revert, save_texts};
use tolk_core::jobs::{self, NewJob};
use tolk_core::layout::DataLayout;
use tolk_core::queue;
use tolk_core::store::Store;
use tolk_core::transcript::{Segment, Transcript};
use tolk_core::StoreError;

fn transcript(n: usize) -> Transcript {
    Transcript {
        version: 1,
        engine: "fake".into(),
        model: "small-q5_1".into(),
        language: "ru".into(),
        duration_ms: n as u64 * 4000,
        segments: (0..n)
            .map(|i| Segment {
                start_ms: i as u64 * 4000,
                end_ms: i as u64 * 4000 + 3500,
                text: format!("Сегмент номер {i}."),
            })
            .collect(),
    }
}

fn setup(n: usize) -> (tempfile::TempDir, Store, String) {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(DataLayout::new(dir.path().join("data"))).unwrap();
    let job = jobs::insert(
        &store.db.conn(),
        &NewJob {
            source_name: "встреча.mp3".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let bytes = transcript(n).to_json().unwrap();
    store
        .files
        .write_atomic(&job.id, "original.json", &bytes)
        .unwrap();
    store
        .files
        .write_atomic(&job.id, "transcript.json", &bytes)
        .unwrap();
    {
        let conn = store.db.conn();
        queue::start(&conn, &job.id).unwrap();
        queue::complete(&conn, &job.id).unwrap();
    }
    (dir, store, job.id)
}

fn texts(store: &Store, id: &str) -> Vec<String> {
    load(store, id)
        .unwrap()
        .segments
        .into_iter()
        .map(|s| s.text)
        .collect()
}

fn has_edits(store: &Store, id: &str) -> bool {
    jobs::get(&store.db.conn(), id).unwrap().unwrap().has_edits
}

#[test]
fn edit_survives_reopening_and_sets_has_edits() {
    let (dir, store, id) = setup(5);
    assert!(!has_edits(&store, &id));
    let mut t = texts(&store, &id);
    t[2] = "Исправленный текст.".into();
    assert!(save_texts(&store, &id, &t).unwrap());
    assert!(has_edits(&store, &id));
    drop(store);

    // «Перезапуск»: база и файлы открываются заново.
    let (store, _) = Store::open(DataLayout::new(dir.path().join("data"))).unwrap();
    let view = load(&store, &id).unwrap();
    assert_eq!(view.segments[2].text, "Исправленный текст.");
    assert!(view.has_edits);
    assert_eq!(view.title, "встреча.mp3");
    // Время сегментов не тронуто.
    assert_eq!(view.segments[2].start_ms, 8000);
    assert_eq!(view.segments[2].end_ms, 11500);
}

#[test]
fn original_is_never_modified_and_revert_restores_it() {
    let (_dir, store, id) = setup(4);
    let original_before = store.files.read(&id, "original.json").unwrap().unwrap();
    let mut t = texts(&store, &id);
    t[0] = "Другой текст".into();
    t[3] = String::new();
    save_texts(&store, &id, &t).unwrap();
    assert_eq!(
        store.files.read(&id, "original.json").unwrap().unwrap(),
        original_before
    );

    let view = revert(&store, &id).unwrap();
    assert!(!view.has_edits);
    assert_eq!(view.segments[0].text, "Сегмент номер 0.");
    assert_eq!(
        store.files.read(&id, "transcript.json").unwrap().unwrap(),
        original_before
    );
    assert!(!has_edits(&store, &id));
}

#[test]
fn editing_back_to_the_original_text_clears_has_edits() {
    let (_dir, store, id) = setup(3);
    let original = texts(&store, &id);
    let mut t = original.clone();
    t[1] = "иначе".into();
    assert!(save_texts(&store, &id, &t).unwrap());
    assert!(!save_texts(&store, &id, &original).unwrap());
    assert!(!has_edits(&store, &id));
}

#[test]
fn invalid_saves_are_rejected_and_change_nothing() {
    let (_dir, store, id) = setup(3);
    let before = store.files.read(&id, "transcript.json").unwrap().unwrap();
    for bad in [
        vec!["один".to_string()],
        vec!["а".into(); 4],
        vec!["x".repeat(100_001), "б".into(), "в".into()],
    ] {
        let err = save_texts(&store, &id, &bad).unwrap_err();
        assert!(matches!(err, StoreError::InvalidInput(_)), "{err:?}");
    }
    assert_eq!(
        store.files.read(&id, "transcript.json").unwrap().unwrap(),
        before
    );
    assert!(!has_edits(&store, &id));
}

#[test]
fn only_completed_jobs_can_be_edited() {
    let (_dir, store, _) = setup(1);
    let queued = jobs::insert(
        &store.db.conn(),
        &NewJob {
            source_name: "в очереди".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        load(&store, &queued.id),
        Err(StoreError::InvalidInput(_))
    ));
    assert!(matches!(
        save_texts(&store, &queued.id, &[]),
        Err(StoreError::InvalidInput(_))
    ));
    assert!(matches!(
        load(&store, "нет такого"),
        Err(StoreError::JobNotFound(_))
    ));
}

#[test]
fn corrupt_working_copy_falls_back_to_the_original() {
    let (_dir, store, id) = setup(2);
    store
        .files
        .write_atomic(&id, "transcript.json", "{ не json".as_bytes())
        .unwrap();
    let view = load(&store, &id).unwrap();
    assert_eq!(view.segments.len(), 2);
    // Сохранение поверх испорченной копии чинит файл.
    let t = texts(&store, &id);
    save_texts(&store, &id, &t).unwrap();
    assert!(
        Transcript::from_json(&store.files.read(&id, "transcript.json").unwrap().unwrap()).is_ok()
    );
}

#[test]
fn interrupted_write_leaves_the_previous_version_intact() {
    let (_dir, store, id) = setup(2);
    let mut t = texts(&store, &id);
    t[0] = "первая правка".into();
    save_texts(&store, &id, &t).unwrap();
    let saved = store.files.read(&id, "transcript.json").unwrap().unwrap();
    // Запись упала между сбросом на диск и переименованием (паника в обработчике).
    let broken = transcript(2).to_json().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store
            .files
            .write_atomic_with_hook(&id, "transcript.json", &broken, || panic!("сбой"))
    }));
    assert!(result.is_err());
    assert_eq!(
        store.files.read(&id, "transcript.json").unwrap().unwrap(),
        saved
    );
    assert_eq!(texts(&store, &id)[0], "первая правка");
}

#[test]
fn long_transcript_of_two_thousand_segments_saves_quickly() {
    let (_dir, store, id) = setup(2000);
    let mut t = texts(&store, &id);
    t[1000] = "правка в середине".into();
    let t0 = std::time::Instant::now();
    save_texts(&store, &id, &t).unwrap();
    let size = fs::metadata(store.files.layout().job_dir(&id).join("transcript.json"))
        .unwrap()
        .len();
    eprintln!(
        "сохранение 2000 сегментов: {:?}, файл {size} байт",
        t0.elapsed()
    );
    assert!(t0.elapsed() < std::time::Duration::from_secs(1));
}
