use std::fs;
use std::io;
use std::path::PathBuf;

use tolk_core::download::DownloadError;
use tolk_core::user_errors::{
    from_download, from_engine, from_store, UserMessage, CODE_INTERRUPTED,
};
use tolk_core::StoreError;

fn repo_file(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Коды ошибок, описанные в протоколе движка (таблица в `docs/ENGINE_PROTOCOL.md`).
fn protocol_codes() -> Vec<String> {
    let doc = repo_file("docs/ENGINE_PROTOCOL.md");
    let section = doc
        .split("## Событие `error`")
        .nth(1)
        .expect("раздел про событие error");
    let table = section.split("\n## ").next().unwrap();
    table
        .lines()
        .filter_map(|l| l.strip_prefix("| `"))
        .filter_map(|l| l.split('`').next())
        .filter(|c| c.chars().all(|ch| ch.is_ascii_uppercase() || ch == '_') && !c.is_empty())
        .map(str::to_string)
        .collect()
}

/// Варианты `ErrorCode` из исходников движка в виде `SCREAMING_SNAKE_CASE` (как в `serde`).
fn engine_enum_codes() -> Vec<String> {
    let src = repo_file("crates/engine/src/error.rs");
    let body = src
        .split("pub enum ErrorCode {")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    body.lines()
        .map(str::trim)
        .filter(|l| l.ends_with(',') && l.chars().next().is_some_and(char::is_uppercase))
        .map(|l| {
            let name = l.trim_end_matches(',');
            let mut out = String::new();
            for (i, c) in name.chars().enumerate() {
                if c.is_uppercase() && i > 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_uppercase());
            }
            out
        })
        .collect()
}

fn assert_complete(m: &UserMessage, what: &str) {
    assert!(
        !m.code.is_empty() && !m.message.is_empty() && !m.action.is_empty(),
        "{what}: {m:?}"
    );
    for text in [&m.message, &m.action] {
        assert!(
            text.chars()
                .any(|c| ('а'..='я').contains(&c.to_ascii_lowercase()) || c == 'ё'),
            "{what}: нужен русский текст: {text}"
        );
        assert_eq!(text.trim(), text, "{what}: лишние пробелы");
    }
}

#[test]
fn every_engine_error_code_has_a_message() {
    let documented = protocol_codes();
    let in_source = engine_enum_codes();
    assert!(
        documented.len() >= 9 && in_source.len() >= 9,
        "разбор не нашёл коды: {documented:?} / {in_source:?}"
    );
    for code in documented.iter().chain(in_source.iter()) {
        let m = from_engine(code, Some("AC-3"));
        assert_eq!(
            &m.code, code,
            "{code}: сообщение должно быть именно для этого кода, а не общим"
        );
        assert_complete(&m, code);
        assert!(m.technical.is_none(), "{code}");
    }
    // Документация и исходники согласованы между собой.
    let (mut a, mut b) = (documented.clone(), in_source.clone());
    a.sort();
    b.sort();
    assert_eq!(
        a, b,
        "коды в ENGINE_PROTOCOL.md и в crates/engine/src/error.rs должны совпадать"
    );
}

#[test]
fn unsupported_codec_names_the_codec() {
    assert!(from_engine("UNSUPPORTED_CODEC", Some("AC-3"))
        .message
        .contains("AC-3"));
    let without = from_engine("UNSUPPORTED_CODEC", None);
    assert_complete(&without, "без названия");
    assert!(!without.message.contains("None"));
}

#[test]
fn unknown_engine_code_falls_back_without_showing_the_code() {
    let m = from_engine("SOMETHING_NEW_FROM_FUTURE", None);
    assert_complete(&m, "запасное");
    assert_eq!(m.code, "INTERNAL");
    assert!(!m.message.contains("SOMETHING") && !m.action.contains("SOMETHING"));
    assert!(
        m.technical
            .as_deref()
            .unwrap()
            .contains("SOMETHING_NEW_FROM_FUTURE"),
        "код сохранён для журнала"
    );
    assert_complete(&from_engine(CODE_INTERRUPTED, None), "прерванное задание");
}

fn all_download_errors() -> Vec<DownloadError> {
    vec![
        DownloadError::NoConnection,
        DownloadError::Dns("huggingface.co".into()),
        DownloadError::Tls,
        DownloadError::Timeout,
        DownloadError::Interrupted,
        DownloadError::Forbidden,
        DownloadError::ProxyAuthRequired,
        DownloadError::NotFound,
        DownloadError::RateLimited,
        DownloadError::Server(503),
        DownloadError::UnexpectedStatus(418),
        DownloadError::BadResponse("нет Content-Range"),
        DownloadError::UntrustedRedirect("http://evil.example/x".into()),
        DownloadError::NoSpace {
            needed: 600_000_000,
            available: 1_000,
        },
        DownloadError::SizeMismatch {
            got: 1,
            expected: 2,
        },
        DownloadError::HashMismatch,
        DownloadError::NotACatalogModel,
        DownloadError::Cancelled,
        DownloadError::Io(io::Error::other("os error 5 at path C:\\Users\\x\\a.part")),
    ]
}

#[test]
fn every_download_error_has_a_message_and_network_ones_offer_import() {
    for e in all_download_errors() {
        let m = from_download(&e);
        assert_complete(&m, &format!("{e:?}"));
        assert_eq!(
            e.suggest_import(),
            m.action.contains("импортируйте") || m.action.contains("импорт"),
            "{e:?}: действие {:?}",
            m.action
        );
    }
}

fn all_store_errors() -> Vec<StoreError> {
    vec![
        StoreError::Db(rusqlite::Error::InvalidQuery),
        StoreError::Io(io::Error::other("os error 13 at /Users/x/tolk.db")),
        StoreError::SchemaTooNew {
            found: 9,
            supported: 2,
        },
        StoreError::BadMigrations("версии должны идти подряд"),
        StoreError::NoSpace,
        StoreError::FileBusy("C:\\Users\\x\\transcript.json".into()),
        StoreError::InvalidId("../x".into()),
        StoreError::BadTranscript("expected value at line 1".into()),
        StoreError::Export("zip error".into()),
        StoreError::GlossaryInvalid("Термин не может быть пустым".into()),
        StoreError::GlossaryDuplicate("Jira".into()),
        StoreError::GlossaryNotFound,
        StoreError::JobNotFound("abc".into()),
        StoreError::InvalidTransition {
            id: "abc".into(),
            from: "COMPLETED",
            to: "PROCESSING",
        },
        StoreError::UnknownModel("x".into()),
        StoreError::ModelNotInstalled("x".into()),
        StoreError::ModelInUse("x".into()),
        StoreError::UnknownStatus("DONE".into()),
    ]
}

#[test]
fn every_store_error_has_a_message() {
    for e in all_store_errors() {
        assert_complete(&from_store(&e), &format!("{e:?}"));
    }
}

#[test]
fn raw_rust_errors_paths_and_internals_never_reach_the_user() {
    let leaks = [
        "os error",
        "sqlite",
        "SqliteFailure",
        "InvalidQuery",
        "Os {",
        "panicked",
        "stack",
        "backtrace",
        "C:\\",
        "/Users",
        "../",
        "expected value",
        "zip error",
        "COMPLETED",
        "huggingface.co",
        "evil.example",
        "Error {",
    ];
    let mut messages: Vec<UserMessage> = all_store_errors().iter().map(from_store).collect();
    messages.extend(all_download_errors().iter().map(from_download));
    for m in messages {
        for text in [&m.message, &m.action] {
            for leak in leaks {
                assert!(
                    !text.contains(leak),
                    "в тексте для пользователя утечка {leak:?}: {m:?}"
                );
            }
        }
    }
}

#[test]
fn technical_details_are_kept_for_the_log() {
    let m = from_store(&StoreError::Io(io::Error::other(
        "os error 13 at /Users/x/tolk.db",
    )));
    assert!(m.technical.unwrap().contains("os error 13"));
    let m = from_download(&DownloadError::Io(io::Error::other("disk exploded")));
    assert!(m.technical.unwrap().contains("disk exploded"));
    let m = from_download(&DownloadError::Server(502));
    assert_eq!(m.technical.as_deref(), Some("HTTP 502"));
}
