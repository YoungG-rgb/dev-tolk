use rusqlite::Connection;
use tolk_core::db::Db;
use tolk_core::glossary::{
    self, iso8601_utc, normalize_key, ImportSummary, TermInput, MAX_FILE_BYTES, MAX_TERMS,
};
use tolk_core::StoreError;

fn open() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("tolk.db")).unwrap();
    (dir, db)
}

fn input(term: &str, variants: &[&str]) -> TermInput {
    TermInput {
        term: term.into(),
        variants: variants.iter().map(|v| v.to_string()).collect(),
        note: String::new(),
        active: true,
    }
}

fn names(conn: &Connection) -> Vec<String> {
    glossary::list(conn, None)
        .unwrap()
        .into_iter()
        .map(|t| t.term)
        .collect()
}

fn file(terms: &str) -> Vec<u8> {
    format!(r#"{{"format":"tolk-glossary","version":1,"exported_at":"2026-10-18T00:00:00Z","terms":{terms}}}"#).into_bytes()
}

#[test]
fn normalization_ignores_case_yo_and_spaces() {
    assert_eq!(normalize_key("  Ёлка   ГЛАВНАЯ "), "елка главная");
    assert_eq!(normalize_key("Jira"), normalize_key("jIRA"));
}

#[test]
fn add_edit_toggle_delete_and_search() {
    let (_d, db) = open();
    let conn = db.conn();
    let k = glossary::add(
        &conn,
        &input(
            "  Kubernetes ",
            &["кубернетес", "Кубер", "кубер", "kubernetes", ""],
        ),
    )
    .unwrap();
    assert_eq!(k.term, "Kubernetes");
    assert_eq!(
        k.variants,
        vec!["кубернетес", "Кубер"],
        "дубли и сам термин в вариантах убираются"
    );
    assert!(k.active);
    glossary::add(&conn, &input("Jira", &["джира"])).unwrap();

    assert_eq!(
        glossary::list(&conn, Some("КУБЕР")).unwrap().len(),
        1,
        "поиск по варианту без учёта регистра"
    );
    assert_eq!(glossary::list(&conn, Some("jir")).unwrap().len(), 1);
    assert_eq!(glossary::list(&conn, Some("")).unwrap().len(), 2);
    assert_eq!(names(&conn), vec!["Jira", "Kubernetes"], "по алфавиту");

    let edited = glossary::update(
        &conn,
        k.id,
        &TermInput {
            note: "оркестратор".into(),
            ..input("Kubernetes", &["кубернетес"])
        },
    )
    .unwrap();
    assert_eq!(
        (edited.note.as_str(), edited.variants.len()),
        ("оркестратор", 1)
    );
    assert!(edited.updated_at >= k.updated_at);

    glossary::set_active(&conn, k.id, false).unwrap();
    let active: Vec<_> = glossary::active_terms(&conn)
        .unwrap()
        .into_iter()
        .map(|t| t.term)
        .collect();
    assert_eq!(
        active,
        vec!["Jira"],
        "неактивные не используются при распознавании"
    );

    assert!(glossary::delete(&conn, k.id).unwrap());
    assert!(!glossary::delete(&conn, k.id).unwrap());
    assert_eq!(names(&conn), vec!["Jira"]);
    assert!(matches!(
        glossary::set_active(&conn, k.id, true),
        Err(StoreError::GlossaryNotFound)
    ));
}

#[test]
fn duplicates_are_detected_after_normalization() {
    let (_d, db) = open();
    let conn = db.conn();
    let jira = glossary::add(&conn, &input("Jira", &[])).unwrap();
    assert!(matches!(
        glossary::add(&conn, &input("  JIRA ", &[])),
        Err(StoreError::GlossaryDuplicate(_))
    ));
    let yo = glossary::add(&conn, &input("Ёж", &[])).unwrap();
    assert!(matches!(
        glossary::add(&conn, &input("еж", &[])),
        Err(StoreError::GlossaryDuplicate(_))
    ));
    assert!(matches!(
        glossary::update(&conn, yo.id, &input("jira", &[])),
        Err(StoreError::GlossaryDuplicate(_))
    ));
    glossary::update(&conn, jira.id, &input("JIRA", &[])).unwrap();
}

#[test]
fn field_limits_are_enforced() {
    let (_d, db) = open();
    let conn = db.conn();
    let bad = [
        input("", &[]),
        input("   ", &[]),
        input(&"я".repeat(101), &[]),
        input("a\u{0}b", &[]),
        input("ok", &["x".repeat(101).as_str()]),
        TermInput {
            note: "н".repeat(501),
            ..input("ok", &[])
        },
    ];
    for b in bad {
        assert!(
            matches!(
                glossary::add(&conn, &b),
                Err(StoreError::GlossaryInvalid(_))
            ),
            "{b:?}"
        );
    }
    let many: Vec<String> = (0..21).map(|i| format!("в{i}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    assert!(matches!(
        glossary::add(&conn, &input("ok", &refs)),
        Err(StoreError::GlossaryInvalid(_))
    ));
    assert_eq!(glossary::count(&conn).unwrap(), 0);
}

#[test]
fn export_then_import_on_another_machine_gives_same_set() {
    let (_d1, a) = open();
    {
        let conn = a.conn();
        glossary::add(
            &conn,
            &TermInput {
                note: "трекер задач".into(),
                ..input("Jira", &["джира", "джиру"])
            },
        )
        .unwrap();
        glossary::add(
            &conn,
            &TermInput {
                active: false,
                ..input("Kubernetes", &["кубер"])
            },
        )
        .unwrap();
        glossary::add(&conn, &input("Ёжик-сервис", &[])).unwrap();
    }
    let bytes = glossary::export_json(&a.conn()).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        text.contains(r#""format": "tolk-glossary""#)
            && text.contains(r#""version": 1"#)
            && text.contains("exported_at")
    );
    assert!(text.contains("Ёжик-сервис"), "кириллица не экранируется");

    let (_d2, b) = open();
    let summary = glossary::import_json(&mut b.conn(), &bytes).unwrap();
    assert_eq!(
        summary,
        ImportSummary {
            added: 3,
            merged: 0,
            skipped: 0
        }
    );

    let strip = |db: &Db| -> Vec<(String, Vec<String>, String, bool)> {
        glossary::list(&db.conn(), None)
            .unwrap()
            .into_iter()
            .map(|t| (t.term, t.variants, t.note, t.active))
            .collect()
    };
    assert_eq!(strip(&a), strip(&b));
}

#[test]
fn importing_the_same_file_twice_duplicates_nothing() {
    let (_d, db) = open();
    let data = file(r#"[{"term":"Jira","variants":["джира"]},{"term":"Kubernetes"}]"#);
    assert_eq!(
        glossary::import_json(&mut db.conn(), &data).unwrap(),
        ImportSummary {
            added: 2,
            merged: 0,
            skipped: 0
        }
    );
    assert_eq!(
        glossary::import_json(&mut db.conn(), &data).unwrap(),
        ImportSummary {
            added: 0,
            merged: 0,
            skipped: 2
        }
    );
    assert_eq!(glossary::count(&db.conn()).unwrap(), 2);
}

#[test]
fn merge_unions_variants_and_keeps_local_spelling_note_and_state() {
    let (_d, db) = open();
    {
        let conn = db.conn();
        glossary::add(
            &conn,
            &TermInput {
                note: "моя заметка".into(),
                active: false,
                ..input("Jira", &["джира"])
            },
        )
        .unwrap();
        glossary::add(&conn, &input("Kafka", &[])).unwrap();
    }
    let data = file(
        r#"[{"term":"JIRA","variants":["Джира","жира","jira cloud"],"note":"чужая заметка","active":true},
            {"term":"kafka","note":"брокер"},
            {"term":"Grafana"}]"#,
    );
    let s = glossary::import_json(&mut db.conn(), &data).unwrap();
    assert_eq!(
        s,
        ImportSummary {
            added: 1,
            merged: 2,
            skipped: 0
        }
    );

    let conn = db.conn();
    let jira = glossary::list(&conn, Some("jira")).unwrap().remove(0);
    assert_eq!(jira.term, "Jira", "каноническое написание — своё");
    assert_eq!(
        jira.variants,
        vec!["джира", "жира", "jira cloud"],
        "варианты объединены без дублей"
    );
    assert_eq!(
        (jira.note.as_str(), jira.active),
        ("моя заметка", false),
        "своя заметка и состояние сохранены"
    );
    let kafka = glossary::list(&conn, Some("kafka")).unwrap().remove(0);
    assert_eq!(
        (kafka.term.as_str(), kafka.note.as_str()),
        ("Kafka", "брокер"),
        "пустая заметка заполняется импортируемой"
    );
}

#[test]
fn duplicates_inside_one_file_are_merged_not_inserted_twice() {
    let (_d, db) = open();
    let data = file(
        r#"[{"term":"Jira","variants":["а"]},{"term":"jira","variants":["б"]},{"term":"JIRA","variants":["а"]}]"#,
    );
    let s = glossary::import_json(&mut db.conn(), &data).unwrap();
    assert_eq!(
        s,
        ImportSummary {
            added: 1,
            merged: 1,
            skipped: 1
        }
    );
    assert_eq!(
        glossary::list(&db.conn(), None).unwrap()[0].variants,
        vec!["а", "б"]
    );
}

#[test]
fn damaged_foreign_or_oversized_files_are_rejected_without_changes() {
    let (_d, db) = open();
    glossary::add(&db.conn(), &input("Jira", &["джира"])).unwrap();
    let before = glossary::list(&db.conn(), None).unwrap();

    let too_many = format!(
        "[{}]",
        (0..=MAX_TERMS)
            .map(|i| format!(r#"{{"term":"t{i}"}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let long_term = format!(r#"[{{"term":"{}"}}]"#, "я".repeat(101));
    let huge = vec![b' '; MAX_FILE_BYTES + 1];
    let valid_then_bad = r#"[{"term":"Хороший"},{"term":""}]"#.to_string();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("пустой файл", b"".to_vec()),
        ("не JSON", b"not json".to_vec()),
        ("чужой JSON", br#"{"hello":"world"}"#.to_vec()),
        ("массив вместо объекта", b"[1,2,3]".to_vec()),
        (
            "другой формат",
            br#"{"format":"other","version":1,"terms":[]}"#.to_vec(),
        ),
        (
            "будущая версия",
            br#"{"format":"tolk-glossary","version":2,"terms":[]}"#.to_vec(),
        ),
        (
            "лишнее поле у термина",
            file(r#"[{"term":"A","evil":"x"}]"#),
        ),
        ("термин не строка", file(r#"[{"term":5}]"#)),
        (
            "варианты не массив",
            file(r#"[{"term":"A","variants":"x"}]"#),
        ),
        ("слишком много терминов", file(&too_many)),
        ("слишком длинный термин", file(&long_term)),
        ("правильный термин, затем пустой", file(&valid_then_bad)),
        ("больше лимита по размеру", huge),
    ];
    for (what, data) in cases {
        let err = glossary::import_json(&mut db.conn(), &data)
            .err()
            .unwrap_or_else(|| panic!("{what}: файл принят"));
        assert!(
            matches!(err, StoreError::GlossaryInvalid(_)),
            "{what}: {err:?}"
        );
        assert!(!err.to_string().is_empty());
        assert_eq!(
            glossary::list(&db.conn(), None).unwrap(),
            before,
            "{what}: словарь изменился"
        );
    }
}

#[test]
fn glossary_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tolk.db");
    {
        let db = Db::open(&path).unwrap();
        glossary::add(&db.conn(), &input("Kubernetes", &["кубер"])).unwrap();
    }
    let db = Db::open(&path).unwrap();
    assert_eq!(names(&db.conn()), vec!["Kubernetes"]);
}

#[test]
fn iso_timestamps_are_correct() {
    assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(iso8601_utc(1_000_000_000_000), "2001-09-09T01:46:40Z");
    assert_eq!(iso8601_utc(1_792_272_000_000), "2026-10-17T21:20:00Z");
    assert_eq!(
        iso8601_utc(951_782_400_000),
        "2000-02-29T00:00:00Z",
        "високосный день"
    );
    assert_eq!(iso8601_utc(-1000), "1969-12-31T23:59:59Z");
}
