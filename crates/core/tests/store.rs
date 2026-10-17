use std::fs;
use std::sync::Arc;
use std::thread;

use rusqlite::Connection;
use tolk_core::db::{Db, Migration, MIGRATIONS};
use tolk_core::jobs::{self, JobStatus, NewJob};
use tolk_core::layout::DataLayout;
use tolk_core::StoreError;

fn new_job(name: &str) -> NewJob {
    NewJob {
        source_name: name.into(),
        audio_path: Some("source.mp3".into()),
        duration_ms: Some(3_600_000),
        model_id: Some("large-v3-turbo-q5_0".into()),
    }
}

#[test]
fn fresh_database_is_created_and_reopens_without_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tolk.db");
    {
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), 1);
        jobs::insert(&db.conn(), &new_job("a.mp3")).unwrap();
    }
    let db = Db::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), 1);
    let count: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "данные пережили повторное открытие");
}

#[test]
fn reliability_pragmas_are_applied() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("tolk.db")).unwrap();
    let conn = db.conn();
    let get = |name: &str| -> String {
        conn.pragma_query_value(None, name, |r| r.get::<_, rusqlite::types::Value>(0))
            .map(|v| format!("{v:?}"))
            .unwrap()
    };
    assert_eq!(get("journal_mode"), "Text(\"wal\")");
    assert_eq!(get("foreign_keys"), "Integer(1)");
    assert_eq!(get("synchronous"), "Integer(1)", "NORMAL");
    assert_eq!(get("busy_timeout"), "Integer(5000)");
}

const V2_ADD_COLUMN: Migration = Migration {
    version: 2,
    sql: "ALTER TABLE jobs ADD COLUMN note TEXT;",
};

#[test]
fn migration_v1_to_v2_preserves_data_on_a_copy() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("tolk.db");
    let id = {
        let db = Db::open(&original).unwrap();
        db.set_setting("language", "ru").unwrap();
        let job = jobs::insert(&db.conn(), &new_job("совещание.m4a")).unwrap();
        job.id
    };
    let copy = dir.path().join("copy.db");
    fs::copy(&original, &copy).unwrap();

    let migrations = [MIGRATIONS[0], V2_ADD_COLUMN];
    let db = Db::open_with(&copy, &migrations).unwrap();
    assert_eq!(db.schema_version().unwrap(), 2);
    let job = jobs::get(&db.conn(), &id)
        .unwrap()
        .expect("задание на месте");
    assert_eq!(job.source_name, "совещание.m4a");
    assert_eq!(db.setting("language").unwrap().as_deref(), Some("ru"));
    db.conn()
        .execute("UPDATE jobs SET note = 'ok' WHERE id = ?1", [&id])
        .unwrap();

    // Исходная база не тронута.
    assert_eq!(Db::open(&original).unwrap().schema_version().unwrap(), 1);
}

#[test]
fn failed_migration_rolls_back_completely() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tolk.db");
    Db::open(&path).unwrap();
    let bad = Migration {
        version: 2,
        sql: "ALTER TABLE jobs ADD COLUMN half_done TEXT; SELECT * FROM no_such_table;",
    };
    let err = Db::open_with(&path, &[MIGRATIONS[0], bad])
        .err()
        .expect("ошибка миграции");
    assert!(matches!(err, StoreError::Db(_)));

    let db = Db::open(&path).unwrap();
    assert_eq!(db.schema_version().unwrap(), 1);
    let cols: Vec<String> = db
        .conn()
        .prepare("PRAGMA table_info(jobs)")
        .unwrap()
        .query_map([], |r| r.get(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        !cols.contains(&"half_done".to_string()),
        "изменения неудачной миграции откатились"
    );
}

#[test]
fn database_from_a_newer_app_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tolk.db");
    Db::open(&path).unwrap();
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    match Db::open(&path) {
        Err(StoreError::SchemaTooNew {
            found: 99,
            supported: 1,
        }) => {}
        other => panic!("ожидалась SchemaTooNew, получено {:?}", other.err()),
    }
}

#[test]
fn migrations_must_be_contiguous() {
    let dir = tempfile::tempdir().unwrap();
    let skipped = Migration {
        version: 3,
        sql: "SELECT 1;",
    };
    let err = Db::open_with(&dir.path().join("x.db"), &[MIGRATIONS[0], skipped])
        .err()
        .unwrap();
    assert!(matches!(err, StoreError::BadMigrations(_)));
}

#[test]
fn schema_stores_metadata_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("tolk.db")).unwrap();
    let conn = db.conn();
    let mut stmt = conn.prepare("SELECT m.name, p.name, p.type FROM sqlite_master m, pragma_table_info(m.name) p WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'").unwrap();
    let cols: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!cols.is_empty());
    for (table, column, ty) in &cols {
        assert!(
            ty == "TEXT" || ty == "INTEGER",
            "{table}.{column}: тип {ty}, BLOB в схеме не нужен"
        );
        for banned in ["transcript", "audio_data", "text_body", "segments"] {
            assert!(
                !column.contains(banned),
                "{table}.{column}: тексты и аудио хранятся файлами"
            );
        }
    }
}

#[test]
fn constraints_reject_invalid_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("tolk.db")).unwrap();
    let conn = db.conn();
    let bad = [
        "UPDATE jobs SET status = 'DONE'",
        "UPDATE jobs SET progress = 101",
        "UPDATE jobs SET progress = -1",
        "UPDATE jobs SET has_edits = 2",
        "UPDATE jobs SET attempts = -1",
        "UPDATE jobs SET duration_ms = -5",
    ];
    jobs::insert(&conn, &new_job("a.mp3")).unwrap();
    for sql in bad {
        assert!(conn.execute(sql, []).is_err(), "{sql} должен отклоняться");
    }
}

#[test]
fn job_roundtrip_and_status_parsing() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("tolk.db")).unwrap();
    let job = jobs::insert(&db.conn(), &new_job("запись.mp3")).unwrap();
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!((job.progress, job.attempts, job.has_edits), (0, 0, false));
    assert_eq!(job.created_at, job.updated_at);
    assert_eq!(job.duration_ms, Some(3_600_000));
    assert_eq!(jobs::get(&db.conn(), &job.id).unwrap().unwrap(), job);
    assert!(jobs::get(&db.conn(), "нет такого").unwrap().is_none());

    for s in [
        JobStatus::Queued,
        JobStatus::Processing,
        JobStatus::Completed,
        JobStatus::Failed,
        JobStatus::Cancelled,
    ] {
        assert_eq!(s.as_str().parse::<JobStatus>().unwrap(), s);
    }
    assert!("DONE".parse::<JobStatus>().is_err());
}

#[test]
fn settings_upsert_and_persist() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tolk.db");
    {
        let db = Db::open(&path).unwrap();
        assert_eq!(db.setting("default_model").unwrap(), None);
        db.set_setting("default_model", "small-q5_1").unwrap();
        db.set_setting("default_model", "large-v3-turbo-q5_0")
            .unwrap();
    }
    assert_eq!(
        Db::open(&path)
            .unwrap()
            .setting("default_model")
            .unwrap()
            .as_deref(),
        Some("large-v3-turbo-q5_0")
    );
}

#[test]
fn concurrent_inserts_from_many_threads() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(&dir.path().join("tolk.db")).unwrap());
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let db = Arc::clone(&db);
            thread::spawn(move || {
                for i in 0..25 {
                    jobs::insert(&db.conn(), &new_job(&format!("t{t}-{i}.mp3"))).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let count: i64 = db
        .conn()
        .query_row("SELECT COUNT(DISTINCT id) FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 200);
}

#[test]
fn data_layout_is_created_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(dir.path().join("data"));
    layout.ensure().unwrap();
    layout.ensure().unwrap();
    for sub in ["models", "jobs", "tmp", "logs"] {
        assert!(layout.root().join(sub).is_dir(), "{sub}");
    }
    assert_eq!(layout.db_path(), layout.root().join("tolk.db"));
    assert_eq!(
        layout.job_dir("abc"),
        layout.root().join("jobs").join("abc")
    );
}
