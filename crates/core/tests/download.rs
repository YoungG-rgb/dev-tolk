mod common;

use std::fs;
use std::path::Path;
use std::time::Duration;

use common::{fake_model, hex, start};
use sha2::{Digest, Sha256};
use tolk_core::download::{
    download, import_from_file_with, is_installed, model_path, partial_bytes, CancelToken,
    DownloadConfig, DownloadError, Progress,
};
use tolk_core::models::ModelEntry;

const LEN: usize = 300_000;

fn cfg() -> DownloadConfig {
    DownloadConfig {
        allowed_hosts: vec!["127.0.0.1".into()],
        https_only: false,
        system_proxy: false,
        connect_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(5),
        chunk_bytes: 64 * 1024,
    }
}

fn entry_at(mut entry: ModelEntry, url: String) -> ModelEntry {
    entry.url = url;
    entry
}

fn run(
    entry: &ModelEntry,
    dir: &Path,
    cfg: &DownloadConfig,
) -> Result<std::path::PathBuf, DownloadError> {
    download(entry, dir, cfg, &CancelToken::new(), &mut |_| {})
}

fn ls(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|d| {
            d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn downloads_verifies_and_installs_atomically() {
    let (data, entry) = fake_model(LEN);
    let server = start(data.clone());
    let entry = entry_at(entry, server.url("/ok"));
    let dir = tempfile::tempdir().unwrap();

    let mut progress: Vec<Progress> = Vec::new();
    let path = download(&entry, dir.path(), &cfg(), &CancelToken::new(), &mut |p| {
        progress.push(*p)
    })
    .unwrap();

    assert_eq!(fs::read(&path).unwrap(), data);
    assert_eq!(
        ls(dir.path()),
        vec!["ggml-test.bin"],
        "остался только итоговый файл, без .part"
    );
    assert!(is_installed(&entry, dir.path()));
    assert!(
        progress
            .windows(2)
            .all(|w| w[0].downloaded <= w[1].downloaded),
        "прогресс не убывает"
    );
    let last = progress.last().unwrap();
    assert_eq!((last.downloaded, last.total), (LEN as u64, LEN as u64));
    assert!(
        server.ranges().len() >= 5,
        "файл скачан блоками: {:?}",
        server.ranges()
    );
}

#[test]
fn installed_model_is_not_downloaded_again() {
    let (data, entry) = fake_model(LEN);
    let server = start(data);
    let entry = entry_at(entry, server.url("/ok"));
    let dir = tempfile::tempdir().unwrap();
    run(&entry, dir.path(), &cfg()).unwrap();
    let hits = server.hit_count();
    run(&entry, dir.path(), &cfg()).unwrap();
    assert_eq!(server.hit_count(), hits, "повторной загрузки нет");
}

#[test]
fn follows_redirects_to_trusted_host_and_keeps_range() {
    let (data, entry) = fake_model(LEN);
    let server = start(data.clone());
    let entry = entry_at(entry, server.url("/redirect"));
    let dir = tempfile::tempdir().unwrap();
    let path = run(&entry, dir.path(), &cfg()).unwrap();
    assert_eq!(fs::read(path).unwrap(), data);
    assert!(
        server.ranges().iter().all(|r| r.starts_with("bytes=")),
        "Range сохранён после редиректа"
    );
}

#[test]
fn redirect_to_untrusted_host_is_rejected() {
    let (data, entry) = fake_model(LEN);
    let server = start(data);
    let entry = entry_at(entry, server.url("/offsite"));
    let dir = tempfile::tempdir().unwrap();
    let err = run(&entry, dir.path(), &cfg()).unwrap_err();
    assert!(
        matches!(err, DownloadError::UntrustedRedirect(_)),
        "{err:?}"
    );
    assert!(err.suggest_import());
    assert!(!model_path(&entry, dir.path()).exists());
}

#[test]
fn catalog_url_on_untrusted_host_or_plain_http_is_refused_before_any_request() {
    let (data, entry) = fake_model(LEN);
    let server = start(data);
    let dir = tempfile::tempdir().unwrap();
    let mut strict = cfg();
    strict.https_only = true;
    let err = run(
        &entry_at(entry.clone(), server.url("/ok")),
        dir.path(),
        &strict,
    )
    .unwrap_err();
    assert!(
        matches!(err, DownloadError::UntrustedRedirect(_)),
        "http при https_only: {err:?}"
    );
    let mut other = cfg();
    other.allowed_hosts = vec!["huggingface.co".into()];
    assert!(run(&entry_at(entry, server.url("/ok")), dir.path(), &other).is_err());
    assert_eq!(server.hit_count(), 0, "запросов не было");
}

#[test]
fn network_cut_then_retry_resumes_from_where_it_stopped() {
    let (data, entry) = fake_model(LEN);
    let server = start(data.clone());
    let entry = entry_at(entry, server.url("/flaky"));
    let dir = tempfile::tempdir().unwrap();

    let err = run(&entry, dir.path(), &cfg()).unwrap_err();
    assert!(matches!(err, DownloadError::Interrupted), "{err:?}");
    assert!(err.is_retryable());
    let kept = partial_bytes(&entry, dir.path());
    assert!(
        kept > 0 && kept < LEN as u64,
        "частичный файл сохранён: {kept}"
    );
    assert!(!model_path(&entry, dir.path()).exists());

    let path = run(&entry, dir.path(), &cfg()).unwrap();
    assert_eq!(
        fs::read(path).unwrap(),
        data,
        "после продолжения файл целый"
    );
    let ranges = server.ranges();
    assert!(
        ranges.iter().any(|r| r
            == &format!(
                "bytes={kept}-{}",
                (kept + 64 * 1024 - 1).min(LEN as u64 - 1)
            )),
        "продолжили с позиции {kept}, а не с нуля: {ranges:?}"
    );
}

#[test]
fn cancel_keeps_partial_file_and_download_can_continue() {
    let (data, entry) = fake_model(LEN);
    let server = start(data.clone());
    let entry = entry_at(entry, server.url("/ok"));
    let dir = tempfile::tempdir().unwrap();

    let cancel = CancelToken::new();
    let c = cancel.clone();
    let err = download(&entry, dir.path(), &cfg(), &cancel, &mut |p| {
        if p.downloaded > 100_000 {
            c.cancel();
        }
    })
    .unwrap_err();
    assert!(matches!(err, DownloadError::Cancelled));
    assert!(partial_bytes(&entry, dir.path()) >= 100_000);
    assert!(!model_path(&entry, dir.path()).exists());

    let path = run(&entry, dir.path(), &cfg()).unwrap();
    assert_eq!(fs::read(path).unwrap(), data);
}

#[test]
fn tampered_file_is_rejected_and_not_installed() {
    let (data, entry) = fake_model(LEN);
    let server = start(data);
    let entry = entry_at(entry, server.url("/corrupt"));
    let dir = tempfile::tempdir().unwrap();
    let err = run(&entry, dir.path(), &cfg()).unwrap_err();
    assert!(matches!(err, DownloadError::HashMismatch), "{err:?}");
    assert!(
        ls(dir.path()).is_empty(),
        "ни итогового файла, ни .part: {:?}",
        ls(dir.path())
    );
}

#[test]
fn truncated_file_on_server_is_not_installed() {
    let (data, entry) = fake_model(LEN);
    let server = start(data);
    let entry = entry_at(entry, server.url("/short"));
    let dir = tempfile::tempdir().unwrap();
    assert!(run(&entry, dir.path(), &cfg()).is_err());
    assert!(!model_path(&entry, dir.path()).exists());
}

#[test]
fn server_ignoring_range_is_handled_by_restarting_from_zero() {
    let (data, entry) = fake_model(LEN);
    let server = start(data.clone());
    let entry = entry_at(entry, server.url("/norange"));
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("ggml-test.bin.part"), &data[..50_000]).unwrap(); // чужой остаток
    let path = run(&entry, dir.path(), &cfg()).unwrap();
    assert_eq!(fs::read(path).unwrap(), data);
}

#[test]
fn wrong_content_range_is_rejected() {
    let (data, entry) = fake_model(LEN);
    let server = start(data);
    let entry = entry_at(entry, server.url("/badrange"));
    let dir = tempfile::tempdir().unwrap();
    let err = run(&entry, dir.path(), &cfg()).unwrap_err();
    assert!(matches!(err, DownloadError::BadResponse(_)), "{err:?}");
}

#[test]
fn oversized_stale_part_is_discarded() {
    let (data, entry) = fake_model(LEN);
    let server = start(data.clone());
    let entry = entry_at(entry, server.url("/ok"));
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("ggml-test.bin.part"), vec![1u8; LEN + 10]).unwrap();
    assert_eq!(
        fs::read(run(&entry, dir.path(), &cfg()).unwrap()).unwrap(),
        data
    );
}

#[test]
fn corrupted_part_from_earlier_attempt_is_caught_by_the_hash() {
    let (data, entry) = fake_model(LEN);
    let server = start(data);
    let entry = entry_at(entry, server.url("/ok"));
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("ggml-test.bin.part"), vec![9u8; 40_000]).unwrap();
    let err = run(&entry, dir.path(), &cfg()).unwrap_err();
    assert!(matches!(err, DownloadError::HashMismatch), "{err:?}");
    assert!(
        ls(dir.path()).is_empty(),
        "испорченное удалено, следующая попытка начнётся с нуля"
    );
}

#[test]
fn http_errors_are_mapped_to_clear_errors_with_import_hint() {
    let (data, entry) = fake_model(LEN);
    let server = start(data);
    let dir = tempfile::tempdir().unwrap();
    type Case = (&'static str, fn(&DownloadError) -> bool, bool);
    let cases: [Case; 5] = [
        (
            "/forbidden",
            |e| matches!(e, DownloadError::Forbidden),
            true,
        ),
        (
            "/proxyauth",
            |e| matches!(e, DownloadError::ProxyAuthRequired),
            true,
        ),
        ("/notfound", |e| matches!(e, DownloadError::NotFound), true),
        ("/busy", |e| matches!(e, DownloadError::Server(503)), true),
        (
            "/ratelimit",
            |e| matches!(e, DownloadError::RateLimited),
            true,
        ),
    ];
    for (path, is_expected, import_hint) in cases {
        let err = run(
            &entry_at(entry.clone(), server.url(path)),
            dir.path(),
            &cfg(),
        )
        .unwrap_err();
        assert!(is_expected(&err), "{path}: {err:?}");
        assert_eq!(err.suggest_import(), import_hint, "{path}");
        assert!(!err.to_string().is_empty());
    }
    assert!(
        ls(dir.path()).iter().all(|n| n.ends_with(".part")),
        "итоговых файлов нет"
    );
}

#[test]
fn stalled_connection_ends_with_timeout() {
    let (data, entry) = fake_model(LEN);
    let server = start(data);
    let entry = entry_at(entry, server.url("/stall"));
    let dir = tempfile::tempdir().unwrap();
    let mut c = cfg();
    c.request_timeout = Duration::from_millis(800);
    let started = std::time::Instant::now();
    let err = run(&entry, dir.path(), &c).unwrap_err();
    assert!(
        matches!(err, DownloadError::Timeout | DownloadError::Interrupted),
        "{err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "не зависает: {:?}",
        started.elapsed()
    );
}

#[test]
fn unreachable_host_and_closed_port_give_dns_and_connection_errors() {
    let (_, entry) = fake_model(LEN);
    let dir = tempfile::tempdir().unwrap();

    let mut c = cfg();
    c.allowed_hosts = vec!["models.invalid".into()];
    let err = run(
        &entry_at(entry.clone(), "http://models.invalid/x".into()),
        dir.path(),
        &c,
    )
    .unwrap_err();
    assert!(
        matches!(err, DownloadError::Dns(ref h) if h == "models.invalid"),
        "{err:?}"
    );
    assert!(err.suggest_import());

    // Порт, на котором никто не слушает.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = run(
        &entry_at(entry, format!("http://127.0.0.1:{port}/x")),
        dir.path(),
        &cfg(),
    )
    .unwrap_err();
    assert!(matches!(err, DownloadError::NoConnection), "{err:?}");
    assert!(err.suggest_import() && err.is_retryable());
}

#[test]
fn failed_tls_handshake_is_reported_as_tls_error() {
    // «Сервер», который отвечает на ClientHello открытым текстом и закрывает соединение.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Ok((mut conn, _)) = listener.accept() {
            let mut buf = [0u8; 512];
            let _ = conn.read(&mut buf);
            let _ = conn.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        }
    });
    let (_, entry) = fake_model(LEN);
    let dir = tempfile::tempdir().unwrap();
    let mut c = cfg();
    c.https_only = true;
    let err = run(
        &entry_at(entry, format!("https://{addr}/ok")),
        dir.path(),
        &c,
    )
    .unwrap_err();
    assert!(matches!(err, DownloadError::Tls), "{err:?}");
    assert!(err.suggest_import());
}

#[test]
fn not_enough_disk_space_is_reported_before_downloading() {
    let (data, mut entry) = fake_model(LEN);
    let server = start(data);
    entry.size = 900_000_000_000_000; // заведомо больше любого диска
    let entry = entry_at(entry, server.url("/ok"));
    let dir = tempfile::tempdir().unwrap();
    let err = run(&entry, dir.path(), &cfg()).unwrap_err();
    assert!(matches!(err, DownloadError::NoSpace { .. }), "{err:?}");
    assert_eq!(server.hit_count(), 0);
}

// ---- импорт из файла ------------------------------------------------------------------------------------

#[test]
fn import_installs_a_catalog_model_without_network_and_leaves_source_untouched() {
    let (data, entry) = fake_model(LEN);
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("флешка").join("модель.bin");
    fs::create_dir_all(src.parent().unwrap()).unwrap();
    fs::write(&src, &data).unwrap();
    let models = dir.path().join("models");

    let entries = [entry.clone()];
    let got = import_from_file_with(&entries, &src, &models, &CancelToken::new()).unwrap();
    assert_eq!(got.id, "test-model");
    assert_eq!(fs::read(model_path(&entry, &models)).unwrap(), data);
    assert_eq!(fs::read(&src).unwrap(), data, "исходный файл не изменён");
    assert_eq!(ls(&models), vec!["ggml-test.bin"], "временных файлов нет");
    assert!(is_installed(&entry, &models));

    // Повторный импорт того же файла безопасен.
    import_from_file_with(&entries, &src, &models, &CancelToken::new()).unwrap();
}

#[test]
fn import_rejects_unknown_corrupted_and_wrong_size_files() {
    let (data, entry) = fake_model(LEN);
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models");
    let entries = [entry];

    let same_size_other_content: Vec<u8> = data.iter().map(|b| b.wrapping_add(1)).collect();
    let cases: [(&str, Vec<u8>); 3] = [
        ("подмена", same_size_other_content),
        ("усечён", data[..LEN - 1].to_vec()),
        ("пустой", Vec::new()),
    ];
    for (what, bytes) in cases {
        let src = dir.path().join("x.bin");
        fs::write(&src, &bytes).unwrap();
        let err = import_from_file_with(&entries, &src, &models, &CancelToken::new()).unwrap_err();
        assert!(
            matches!(err, DownloadError::NotACatalogModel),
            "{what}: {err:?}"
        );
        assert!(
            ls(&models).is_empty(),
            "{what}: в каталоге моделей ничего не осталось: {:?}",
            ls(&models)
        );
    }
    assert!(import_from_file_with(
        &entries,
        &dir.path().join("нет.bin"),
        &models,
        &CancelToken::new()
    )
    .is_err());
}

#[test]
fn import_can_be_cancelled_and_leaves_nothing() {
    let (data, entry) = fake_model(LEN);
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("m.bin");
    fs::write(&src, &data).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    let models = dir.path().join("models");
    let err = import_from_file_with(&[entry], &src, &models, &cancel).unwrap_err();
    assert!(matches!(err, DownloadError::Cancelled));
    assert!(ls(&models).is_empty());
}

#[test]
fn test_helpers_hash_matches() {
    let (data, entry) = fake_model(1000);
    assert_eq!(entry.sha256, hex(&Sha256::digest(&data)));
}
