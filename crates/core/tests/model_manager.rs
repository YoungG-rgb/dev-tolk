use std::fs::{self, File};
use std::path::Path;

use tolk_core::db::Db;
use tolk_core::jobs::{self, NewJob};
use tolk_core::model_manager::{
    default_model, delete_model, list, memory_warning, model_for_new_job, needs_first_run,
    set_default_model, MemoryWarning,
};
use tolk_core::models::{find, SystemInfo, DEFAULT_MODEL_ID, VAD_MODEL_ID};
use tolk_core::{queue, StoreError};

const GIB: u64 = 1024 * 1024 * 1024;

/// «Устанавливает» модель: файл нужного размера (разреженный, содержимое не важно: установленной
/// считается модель с правильным размером, сумму проверяет загрузчик при установке).
fn install(dir: &Path, id: &str) {
    let entry = find(id).unwrap();
    let f = File::create(dir.join(&entry.file)).unwrap();
    f.set_len(entry.size).unwrap();
}

fn setup() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("models")).unwrap();
    let db = Db::open(&dir.path().join("tolk.db")).unwrap();
    (dir, db)
}

fn system(available: u64) -> SystemInfo {
    SystemInfo {
        os: "macos",
        arch: "aarch64",
        total_ram_bytes: 16 * GIB,
        available_ram_bytes: available,
        avx2: None,
    }
}

#[test]
fn first_run_is_needed_until_a_recognition_model_is_installed() {
    let (dir, _db) = setup();
    let models = dir.path().join("models");
    assert!(needs_first_run(&models));
    install(&models, VAD_MODEL_ID);
    assert!(
        needs_first_run(&models),
        "одной модели определения речи недостаточно"
    );
    install(&models, "small-q5_1");
    assert!(!needs_first_run(&models));
    assert!(needs_first_run(&dir.path().join("нет каталога")));
}

#[test]
fn list_shows_status_default_and_recommendation_without_the_vad_component() {
    let (dir, db) = setup();
    let models = dir.path().join("models");
    install(&models, DEFAULT_MODEL_ID);
    install(&models, VAD_MODEL_ID);
    fs::write(models.join("ggml-small-q5_1.bin.part"), vec![0u8; 12345]).unwrap();
    set_default_model(&db, &models, DEFAULT_MODEL_ID).unwrap();

    let all = list(&db, &models, &system(8 * GIB)).unwrap();
    assert!(all.iter().all(|s| s.entry.id != VAD_MODEL_ID));
    let turbo = all.iter().find(|s| s.entry.id == DEFAULT_MODEL_ID).unwrap();
    assert!(turbo.installed && turbo.is_default && turbo.recommended);
    let small = all.iter().find(|s| s.entry.id == "small-q5_1").unwrap();
    assert!(!small.installed && !small.is_default && !small.recommended);
    assert_eq!(small.partial_bytes, 12345);
    assert_eq!(all.iter().filter(|s| s.is_default).count(), 1);
}

#[test]
fn default_model_must_be_an_installed_recognition_model() {
    let (dir, db) = setup();
    let models = dir.path().join("models");
    assert!(matches!(
        set_default_model(&db, &models, "small-q5_1"),
        Err(StoreError::ModelNotInstalled(_))
    ));
    assert!(matches!(
        set_default_model(&db, &models, "нет такой"),
        Err(StoreError::UnknownModel(_))
    ));
    install(&models, VAD_MODEL_ID);
    assert!(
        matches!(
            set_default_model(&db, &models, VAD_MODEL_ID),
            Err(StoreError::UnknownModel(_))
        ),
        "VAD не выбирается"
    );
    assert!(default_model(&db, &models).unwrap().is_none());

    install(&models, "small-q5_1");
    set_default_model(&db, &models, "small-q5_1").unwrap();
    assert_eq!(
        default_model(&db, &models).unwrap().unwrap().id,
        "small-q5_1"
    );

    // Выбор переживает перезапуск, но исчезает вместе с файлом модели.
    fs::remove_file(models.join("ggml-small-q5_1.bin")).unwrap();
    assert!(default_model(&db, &models).unwrap().is_none());
}

#[test]
fn changing_default_applies_to_new_jobs_only() {
    let (dir, db) = setup();
    let models = dir.path().join("models");
    assert_eq!(
        model_for_new_job(&db, &models).unwrap(),
        None,
        "без моделей задание не получает модель"
    );

    // Модель выбирается до блокировки соединения: guard `db.conn()` нельзя держать при обращении к `db` внутри аргументов.
    let new_job = |name: &str| {
        let model_id = model_for_new_job(&db, &models).unwrap();
        let conn = db.conn();
        jobs::insert(
            &conn,
            &NewJob {
                source_name: name.into(),
                model_id,
                ..NewJob::default()
            },
        )
        .unwrap()
    };

    install(&models, "small-q5_1");
    assert_eq!(
        model_for_new_job(&db, &models).unwrap().as_deref(),
        Some("small-q5_1"),
        "первая установленная становится выбранной"
    );
    let first = new_job("a");

    install(&models, DEFAULT_MODEL_ID);
    set_default_model(&db, &models, DEFAULT_MODEL_ID).unwrap();
    let second = new_job("b");

    assert_eq!(
        jobs::get(&db.conn(), &first.id)
            .unwrap()
            .unwrap()
            .model_id
            .as_deref(),
        Some("small-q5_1"),
        "старое задание не изменилось"
    );
    assert_eq!(second.model_id.as_deref(), Some(DEFAULT_MODEL_ID));
}

#[test]
fn delete_removes_file_and_partial_and_resets_default() {
    let (dir, db) = setup();
    let models = dir.path().join("models");
    install(&models, "small-q5_1");
    fs::write(models.join("ggml-small-q5_1.bin.part"), b"x").unwrap();
    set_default_model(&db, &models, "small-q5_1").unwrap();

    delete_model(&db, &models, "small-q5_1").unwrap();
    assert!(
        !models.join("ggml-small-q5_1.bin").exists()
            && !models.join("ggml-small-q5_1.bin.part").exists()
    );
    assert_eq!(db.setting("default_model").unwrap(), None, "выбор сброшен");
    delete_model(&db, &models, "small-q5_1").unwrap(); // повторное удаление безопасно
}

#[test]
fn model_used_by_a_running_job_cannot_be_deleted() {
    let (dir, db) = setup();
    let models = dir.path().join("models");
    install(&models, "small-q5_1");
    install(&models, DEFAULT_MODEL_ID);
    let job = jobs::insert(
        &db.conn(),
        &NewJob {
            source_name: "a".into(),
            model_id: Some("small-q5_1".into()),
            ..NewJob::default()
        },
    )
    .unwrap();
    queue::start(&db.conn(), &job.id).unwrap();

    assert!(matches!(
        delete_model(&db, &models, "small-q5_1"),
        Err(StoreError::ModelInUse(_))
    ));
    assert!(models.join("ggml-small-q5_1.bin").exists(), "файл остался");
    delete_model(&db, &models, DEFAULT_MODEL_ID).unwrap(); // другая модель удаляется свободно

    queue::complete(&db.conn(), &job.id).unwrap();
    delete_model(&db, &models, "small-q5_1").unwrap();
}

#[test]
fn queued_job_does_not_block_deletion_and_vad_is_protected() {
    let (dir, db) = setup();
    let models = dir.path().join("models");
    install(&models, "small-q5_1");
    install(&models, VAD_MODEL_ID);
    jobs::insert(
        &db.conn(),
        &NewJob {
            source_name: "a".into(),
            model_id: Some("small-q5_1".into()),
            ..NewJob::default()
        },
    )
    .unwrap();
    delete_model(&db, &models, "small-q5_1").unwrap();
    assert!(matches!(
        delete_model(&db, &models, VAD_MODEL_ID),
        Err(StoreError::UnknownModel(_))
    ));
    assert!(matches!(
        delete_model(&db, &models, "нет такой"),
        Err(StoreError::UnknownModel(_))
    ));
    assert!(models.join(&find(VAD_MODEL_ID).unwrap().file).exists());
}

#[test]
fn memory_warning_compares_available_memory_with_estimate_plus_headroom() {
    let turbo = find(DEFAULT_MODEL_ID).unwrap(); // оценка 1200 МБ, с запасом 1800 МБ
    let mb = 1024 * 1024;
    assert_eq!(
        memory_warning(turbo, &system(1000 * mb)),
        Some(MemoryWarning {
            required_mb: 1800,
            available_mb: 1000
        })
    );
    assert_eq!(
        memory_warning(turbo, &system(1799 * mb)).map(|w| w.available_mb),
        Some(1799)
    );
    assert_eq!(memory_warning(turbo, &system(1800 * mb)), None);
    assert_eq!(memory_warning(turbo, &system(8 * GIB)), None);
    assert_eq!(
        memory_warning(find("small-q5_1").unwrap(), &system(0)),
        None,
        "нет оценки — нет предупреждения"
    );
}
