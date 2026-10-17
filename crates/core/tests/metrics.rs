use std::fs;

use tolk_core::db::Db;
use tolk_core::jobs::{self, NewJob};
use tolk_core::layout::DataLayout;
use tolk_core::metrics::{
    build_report, record, save_report, set_rating, JobMetrics, Outcome, REPORT_FORMAT,
    REPORT_VERSION,
};
use tolk_core::models::SystemInfo;
use tolk_core::store::Store;
use tolk_core::StoreError;

const GIB: u64 = 1024 * 1024 * 1024;

fn system() -> SystemInfo {
    SystemInfo {
        os: "macos",
        arch: "aarch64",
        total_ram_bytes: 24 * GIB,
        available_ram_bytes: 10 * GIB,
        avx2: None,
    }
}

fn metrics(audio_ms: u64, processing_ms: u64, outcome: Outcome) -> JobMetrics {
    JobMetrics {
        audio_ms,
        processing_ms,
        peak_rss_bytes: Some(1_500_000_000),
        backend: Some("metal".into()),
        model_id: Some("large-v3-turbo-q5_0".into()),
        outcome,
        error_code: (outcome == Outcome::Failed).then(|| "INTERNAL".to_string()),
    }
}

fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("tolk.db")).unwrap();
    (dir, db)
}

#[test]
fn empty_report_is_valid() {
    let (_d, db) = open();
    let r = build_report(&db.conn(), &system(), "0.1.0").unwrap();
    assert_eq!((r.format, r.version), (REPORT_FORMAT, REPORT_VERSION));
    assert!(r.jobs.is_empty() && r.summaries.is_empty());
    assert_eq!(
        (
            r.system.os.as_str(),
            r.system.arch.as_str(),
            r.system.ram_total_mb
        ),
        ("macos", "aarch64", 24 * 1024)
    );
    assert!(r.generated_at.ends_with('Z'));
}

#[test]
fn report_computes_rtf_memory_and_summaries() {
    let (_d, db) = open();
    let conn = db.conn();
    record(&conn, "a", &metrics(3_600_000, 540_000, Outcome::Success)).unwrap(); // RTF 0,15
    record(&conn, "b", &metrics(1_800_000, 360_000, Outcome::Success)).unwrap(); // RTF 0,2
    record(&conn, "c", &metrics(2_400_000, 960_000, Outcome::Success)).unwrap(); // RTF 0,4
    record(&conn, "d", &metrics(600_000, 5_000, Outcome::Failed)).unwrap();
    let mut cpu = metrics(1_000_000, 500_000, Outcome::Success);
    cpu.backend = Some("cpu".into());
    cpu.peak_rss_bytes = None;
    record(&conn, "e", &cpu).unwrap();

    let r = build_report(&conn, &system(), "0.1.0").unwrap();
    assert_eq!(r.jobs.len(), 5);
    assert_eq!(r.jobs[0].rtf, Some(0.15));
    assert_eq!(
        r.jobs[0].peak_rss_mb,
        Some(1431),
        "1 500 000 000 Б с округлением вверх до МиБ"
    );
    assert_eq!(r.jobs[3].outcome, "FAILED");
    assert_eq!(r.jobs[3].error_code.as_deref(), Some("INTERNAL"));
    assert_eq!(r.jobs[4].peak_rss_mb, None);

    let metal = r
        .summaries
        .iter()
        .find(|s| s.backend.as_deref() == Some("metal"))
        .unwrap();
    assert_eq!((metal.jobs, metal.succeeded), (4, 3));
    assert_eq!(
        metal.rtf_median,
        Some(0.2),
        "медиана только по успешным заданиям"
    );
    assert_eq!(metal.rtf_max, Some(0.4));
    assert_eq!(metal.peak_rss_mb_max, Some(1431));
    let cpu = r
        .summaries
        .iter()
        .find(|s| s.backend.as_deref() == Some("cpu"))
        .unwrap();
    assert_eq!((cpu.jobs, cpu.rtf_median), (1, Some(0.5)));
}

#[test]
fn even_number_of_jobs_uses_the_mean_of_the_two_middle_values() {
    let (_d, db) = open();
    let conn = db.conn();
    record(&conn, "a", &metrics(1000, 100, Outcome::Success)).unwrap();
    record(&conn, "b", &metrics(1000, 300, Outcome::Success)).unwrap();
    let r = build_report(&conn, &system(), "0.1.0").unwrap();
    assert_eq!(r.summaries[0].rtf_median, Some(0.2));
}

#[test]
fn zero_length_audio_does_not_divide_by_zero() {
    let (_d, db) = open();
    record(&db.conn(), "a", &metrics(0, 10, Outcome::Failed)).unwrap();
    let r = build_report(&db.conn(), &system(), "0.1.0").unwrap();
    assert_eq!(r.jobs[0].rtf, None);
}

#[test]
fn rerun_replaces_metrics_and_resets_rating() {
    let (_d, db) = open();
    let conn = db.conn();
    record(&conn, "a", &metrics(1000, 900, Outcome::Failed)).unwrap();
    set_rating(&conn, "a", 4).unwrap();
    record(&conn, "a", &metrics(1000, 200, Outcome::Success)).unwrap();
    let r = build_report(&conn, &system(), "0.1.0").unwrap();
    assert_eq!(r.jobs.len(), 1, "повтор заменяет прежнюю запись");
    assert_eq!(
        (r.jobs[0].outcome.as_str(), r.jobs[0].user_rating),
        ("SUCCESS", None)
    );
    set_rating(&conn, "a", 5).unwrap();
    assert_eq!(
        build_report(&conn, &system(), "0.1.0").unwrap().jobs[0].user_rating,
        Some(5)
    );
}

#[test]
fn rating_is_validated() {
    let (_d, db) = open();
    let conn = db.conn();
    record(&conn, "a", &metrics(1000, 100, Outcome::Success)).unwrap();
    for bad in [0u8, 6, 255] {
        assert!(
            matches!(
                set_rating(&conn, "a", bad),
                Err(StoreError::InvalidInput(_))
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        set_rating(&conn, "нет", 3),
        Err(StoreError::JobNotFound(_))
    ));
    for good in 1..=5u8 {
        set_rating(&conn, "a", good).unwrap();
    }
}

#[test]
fn database_rejects_invalid_values() {
    let (_d, db) = open();
    let conn = db.conn();
    for sql in [
        "INSERT INTO job_metrics (job_id, audio_ms, processing_ms, outcome, recorded_at) VALUES ('x', -1, 1, 'SUCCESS', 0)",
        "INSERT INTO job_metrics (job_id, audio_ms, processing_ms, outcome, recorded_at) VALUES ('x', 1, 1, 'DONE', 0)",
        "INSERT INTO job_metrics (job_id, audio_ms, processing_ms, backend, outcome, recorded_at) VALUES ('x', 1, 1, 'gpu', 'SUCCESS', 0)",
        "INSERT INTO job_metrics (job_id, audio_ms, processing_ms, user_rating, outcome, recorded_at) VALUES ('x', 1, 1, 9, 'SUCCESS', 0)",
    ] {
        assert!(conn.execute(sql, []).is_err(), "{sql}");
    }
}

/// Главное требование приватности: в файле отчёта нет имён файлов, текстов, идентификаторов заданий и путей.
#[test]
fn report_contains_no_file_names_texts_ids_or_paths() {
    let dir = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(dir.path().join("секретная-папка"));
    let (store, _) = Store::open(layout).unwrap();
    let secret_name = "СОВЕЩАНИЕ_С_ИВАНОВЫМ_про_зарплаты.m4a";
    let secret_text = "Иванов получит премию в размере миллиона";
    let mut ids = Vec::new();
    for i in 0..3 {
        let job = jobs::insert(
            &store.db.conn(),
            &NewJob {
                source_name: format!("{i}_{secret_name}"),
                model_id: Some("large-v3-turbo-q5_0".into()),
                ..NewJob::default()
            },
        )
        .unwrap();
        store
            .files
            .write_atomic(&job.id, "transcript.json", secret_text.as_bytes())
            .unwrap();
        record(
            &store.db.conn(),
            &job.id,
            &metrics(1_000_000, 200_000, Outcome::Success),
        )
        .unwrap();
        ids.push(job.id);
    }
    let dest = dir.path().join("отчёт.json");
    save_report(&store.db.conn(), &system(), "0.1.0", &dest).unwrap();
    let text = fs::read_to_string(&dest).unwrap();

    let mut forbidden: Vec<String> = vec![
        secret_name.into(),
        "Иванов".into(),
        "премию".into(),
        "секретная-папка".into(),
        "transcript".into(),
        "source".into(),
        ".m4a".into(),
        "/Users".into(),
        "C:\\".into(),
    ];
    forbidden.extend(ids);
    forbidden.push(dir.path().to_string_lossy().into_owned());
    if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
        if user.len() > 2 {
            forbidden.push(user);
        }
    }
    if let Ok(out) = std::process::Command::new("hostname").output() {
        let host = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if host.len() > 2 {
            forbidden.push(host);
        }
    }
    for item in forbidden {
        assert!(!text.contains(&item), "в отчёте есть {item:?}:\n{text}");
    }
}

#[test]
fn metrics_survive_deleting_the_job_and_contain_no_content() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(DataLayout::new(dir.path().join("data"))).unwrap();
    let job = jobs::insert(
        &store.db.conn(),
        &NewJob {
            source_name: "a.mp3".into(),
            ..NewJob::default()
        },
    )
    .unwrap();
    record(
        &store.db.conn(),
        &job.id,
        &metrics(1000, 100, Outcome::Success),
    )
    .unwrap();
    assert!(store.delete_job(&job.id).unwrap());
    assert_eq!(
        build_report(&store.db.conn(), &system(), "0.1.0")
            .unwrap()
            .jobs
            .len(),
        1,
        "метрики остаются после удаления записи"
    );
}

#[test]
fn saved_report_is_valid_json_written_atomically() {
    let (dir, db) = open();
    record(&db.conn(), "a", &metrics(60_000, 9_000, Outcome::Success)).unwrap();
    let dest = dir.path().join("Отчёт о производительности.json");
    save_report(&db.conn(), &system(), "0.1.0", &dest).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&dest).unwrap()).unwrap();
    assert_eq!(value["format"], REPORT_FORMAT);
    assert_eq!(value["jobs"][0]["rtf"], 0.15);
    assert_eq!(value["system"]["ram_total_mb"], 24 * 1024);
    assert!(fs::read_dir(dir.path()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".part")));
    assert!(save_report(
        &db.conn(),
        &system(),
        "0.1.0",
        &dir.path().join("нет").join("r.json")
    )
    .is_err());
}
