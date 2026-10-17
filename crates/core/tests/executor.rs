//! Исполнитель очереди на фиктивном движке (bash-скрипт, имитирующий протокол): порядок, отмена,
//! сбои, прерывание при закрытии. Только Unix.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tolk_core::executor::{Executor, JobEvent};
use tolk_core::jobs::{self, Job, JobStatus, NewJob};
use tolk_core::layout::DataLayout;
use tolk_core::models::{find, VAD_MODEL_ID};
use tolk_core::power::SleepInhibitor;
use tolk_core::queue;
use tolk_core::store::Store;
use tolk_core::{metrics, StoreError};

const MODEL: &str = "small-q5_1";

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: Arc<Store>,
    events: Arc<Mutex<Vec<JobEvent>>>,
    inhibitor: Arc<Mutex<SleepInhibitor>>,
}

fn install(models: &Path, id: &str) {
    let entry = find(id).unwrap();
    fs::File::create(models.join(&entry.file))
        .unwrap()
        .set_len(entry.size)
        .unwrap();
}

fn new_env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (store, _) = Store::open(DataLayout::new(root.join("data"))).unwrap();
    let models = root.join("data").join("models");
    install(&models, MODEL);
    install(&models, VAD_MODEL_ID);
    Env {
        _dir: dir,
        root,
        store: Arc::new(store),
        events: Arc::default(),
        inhibitor: Arc::default(),
    }
}

impl Env {
    fn executor(&self, exe: &Path) -> Arc<Executor> {
        let events = Arc::clone(&self.events);
        Executor::new(
            Arc::clone(&self.store),
            exe.to_path_buf(),
            Arc::clone(&self.inhibitor),
            Arc::new(move |e| events.lock().unwrap().push(e)),
        )
    }

    fn add_job(&self, name: &str, model: Option<&str>) -> Job {
        let job = {
            let conn = self.store.db.conn();
            jobs::insert(
                &conn,
                &NewJob {
                    source_name: name.into(),
                    audio_path: Some("source.wav".into()),
                    duration_ms: Some(1000),
                    model_id: model.map(str::to_string),
                },
            )
            .unwrap()
        };
        let src = self.root.join(format!("{}.wav", job.id));
        fs::write(&src, b"RIFF").unwrap();
        self.store
            .files
            .copy_audio(&job.id, &src, "source.wav")
            .unwrap();
        job
    }

    fn job(&self, id: &str) -> Job {
        let conn = self.store.db.conn();
        jobs::get(&conn, id).unwrap().unwrap()
    }

    fn wait_status(&self, id: &str, status: JobStatus, timeout: Duration) -> Job {
        let t0 = Instant::now();
        loop {
            let job = self.job(id);
            if job.status == status {
                return job;
            }
            assert!(
                t0.elapsed() < timeout,
                "ждали {status:?}, статус {:?}",
                job.status
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn started_ids(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                JobEvent::Started { job_id } => Some(job_id.clone()),
                _ => None,
            })
            .collect()
    }
}

const RESULT: &str = r#"{"version":1,"engine":"fake","model":"small-q5_1","language":"ru","duration_ms":1000,"segments":[{"start_ms":0,"end_ms":900,"text":"раз"}]}"#;
const DONE: &str = r#"{"type":"done","output":"x","segments":1,"audio_ms":1000,"elapsed_ms":10,"model_load_ms":1,"decode_ms":1,"recognize_ms":8,"peak_rss_bytes":123456,"backend":"cpu"}"#;

/// Фиктивный движок. Пока существует файл `slow` рядом со скриптом, «работает» 10 секунд (отзывчиво к
/// SIGTERM); иначе заканчивает сразу. `result` — содержимое выходного файла.
fn fake_engine(dir: &Path, result: &str, slow: bool) -> PathBuf {
    let exe = dir.join("engine.sh");
    let flag = dir.join("slow");
    if slow {
        fs::write(&flag, b"").unwrap();
    }
    let body = format!(
        r#"#!/bin/bash
out=""
while [ $# -gt 0 ]; do [ "$1" = "--output" ] && out="$2"; shift; done
echo '{{"type":"progress","percent":0}}'
if [ -f '{flag}' ]; then for i in $(seq 100); do sleep 0.1; done; fi
cat > "$out" <<'RESULT_EOF'
{result}
RESULT_EOF
echo '{{"type":"progress","percent":100}}'
echo '{DONE}'
"#,
        flag = flag.display()
    );
    fs::write(&exe, body).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

fn script(dir: &Path, body: &str) -> PathBuf {
    let exe = dir.join("custom.sh");
    fs::write(&exe, format!("#!/bin/bash\n{body}\n")).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

const WAIT: Duration = Duration::from_secs(30);

#[test]
fn six_queued_files_are_processed_in_order_without_intervention() {
    let env = new_env();
    let exe = fake_engine(&env.root, RESULT, false);
    let jobs: Vec<Job> = (0..6)
        .map(|i| env.add_job(&format!("запись {i}.mp3"), Some(MODEL)))
        .collect();
    let ex = env.executor(&exe);
    let handle = ex.spawn();
    for j in &jobs {
        let done = env.wait_status(&j.id, JobStatus::Completed, WAIT);
        assert_eq!(done.progress, 100);
        let original = env.store.files.read(&j.id, "original.json").unwrap();
        let work = env.store.files.read(&j.id, "transcript.json").unwrap();
        assert!(original.is_some());
        assert_eq!(work, original, "рабочая копия начинается с оригинала");
    }
    assert_eq!(
        env.started_ids(),
        jobs.iter().map(|j| j.id.clone()).collect::<Vec<_>>(),
        "порядок добавления"
    );
    let report = {
        let conn = env.store.db.conn();
        metrics::build_report(&conn, &tolk_core::models::SystemInfo::detect(), "test").unwrap()
    };
    assert_eq!(report.jobs.len(), 6);
    assert!(report
        .jobs
        .iter()
        .all(|j| j.outcome == "SUCCESS" && j.peak_rss_mb == Some(1)));
    assert!(
        !env.inhibitor.lock().unwrap().is_active(),
        "запрет сна снят после очереди"
    );
    assert!(
        fs::read_dir(env.root.join("data").join("tmp"))
            .unwrap()
            .next()
            .is_none(),
        "временных файлов не осталось"
    );
    ex.shutdown();
    handle.join().unwrap();
}

#[test]
fn two_executors_never_run_a_job_twice_or_in_parallel() {
    let env = new_env();
    let exe = fake_engine(&env.root, RESULT, false);
    let jobs: Vec<Job> = (0..6)
        .map(|i| env.add_job(&format!("{i}.mp3"), Some(MODEL)))
        .collect();
    let (a, b) = (env.executor(&exe), env.executor(&exe));
    let (ha, hb) = (a.spawn(), b.spawn());
    for j in &jobs {
        env.wait_status(&j.id, JobStatus::Completed, WAIT);
    }
    let mut started = env.started_ids();
    assert_eq!(started.len(), 6, "каждое задание запущено один раз");
    started.sort();
    started.dedup();
    assert_eq!(started.len(), 6);
    for j in &jobs {
        assert_eq!(env.job(&j.id).attempts, 1);
    }
    a.shutdown();
    b.shutdown();
    ha.join().unwrap();
    hb.join().unwrap();
}

#[test]
fn cancelling_a_queued_job_never_starts_it() {
    let env = new_env();
    let exe = fake_engine(&env.root, RESULT, true);
    let first = env.add_job("первое.mp3", Some(MODEL));
    let second = env.add_job("второе.mp3", Some(MODEL));
    let ex = env.executor(&exe);
    let handle = ex.spawn();
    env.wait_status(&first.id, JobStatus::Processing, WAIT);
    let cancelled = ex.cancel_job(&second.id).unwrap();
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    ex.cancel_job(&first.id).unwrap();
    env.wait_status(&first.id, JobStatus::Cancelled, WAIT);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(env.started_ids(), vec![first.id.clone()]);
    assert_eq!(env.job(&second.id).attempts, 0);
    ex.shutdown();
    handle.join().unwrap();
}

#[test]
fn cancelling_a_running_job_stops_the_engine_quickly_and_allows_retry() {
    let env = new_env();
    let exe = fake_engine(&env.root, RESULT, true);
    let job = env.add_job("длинная.mp3", Some(MODEL));
    let ex = env.executor(&exe);
    let handle = ex.spawn();
    env.wait_status(&job.id, JobStatus::Processing, WAIT);
    assert!(ex.is_running(&job.id));
    let t0 = Instant::now();
    ex.cancel_job(&job.id).unwrap();
    ex.cancel_job(&job.id).unwrap(); // повторный клик безвреден
    let done = env.wait_status(&job.id, JobStatus::Cancelled, WAIT);
    assert!(
        t0.elapsed() < Duration::from_secs(3),
        "отмена заняла {:?}",
        t0.elapsed()
    );
    assert_eq!(done.error_code, None);
    assert!(env
        .store
        .files
        .read(&job.id, "original.json")
        .unwrap()
        .is_none());
    assert!(!ex.is_running(&job.id));

    // Повтор: двойной клик «Повторить» ставит в очередь один раз.
    fs::remove_file(env.root.join("slow")).unwrap();
    let first_retry = queue::retry(&env.store.db.conn(), &job.id);
    assert!(first_retry.is_ok());
    let second_retry = queue::retry(&env.store.db.conn(), &job.id);
    // Либо запрещённый переход, либо исполнитель уже взял задание: в любом случае повторной постановки нет.
    assert!(matches!(
        second_retry,
        Err(StoreError::InvalidTransition { .. })
    ));
    ex.notify();
    let ok = env.wait_status(&job.id, JobStatus::Completed, WAIT);
    assert_eq!(ok.attempts, 2);
    ex.shutdown();
    handle.join().unwrap();
}

#[test]
fn missing_model_and_missing_audio_fail_with_russian_messages() {
    let env = new_env();
    let exe = fake_engine(&env.root, RESULT, false);
    let no_model = env.add_job("а.mp3", Some("large-v3-q5_0")); // не установлена
    let no_audio = env.add_job("б.mp3", Some(MODEL));
    fs::remove_file(
        env.store
            .files
            .layout()
            .job_dir(&no_audio.id)
            .join("source.wav"),
    )
    .unwrap();
    let ex = env.executor(&exe);
    let handle = ex.spawn();
    let a = env.wait_status(&no_model.id, JobStatus::Failed, WAIT);
    assert_eq!(a.error_code.as_deref(), Some("MODEL_MISSING"));
    assert!(a.error_message.unwrap().contains("модели"));
    let b = env.wait_status(&no_audio.id, JobStatus::Failed, WAIT);
    assert_eq!(b.error_code.as_deref(), Some("FILE_NOT_FOUND"));
    ex.shutdown();
    handle.join().unwrap();
}

#[test]
fn engine_crash_fails_the_job_not_the_queue() {
    let env = new_env();
    let crash = script(
        &env.root,
        "echo '{\"type\":\"progress\",\"percent\":3}'; exit 9",
    );
    let first = env.add_job("упал.mp3", Some(MODEL));
    let ex = env.executor(&crash);
    let handle = ex.spawn();
    let failed = env.wait_status(&first.id, JobStatus::Failed, WAIT);
    assert_eq!(failed.error_code.as_deref(), Some("INTERNAL"));
    // Очередь жива: следующее задание берётся (и тоже падает, но не зависает).
    let second = env.add_job("ещё.mp3", Some(MODEL));
    ex.notify();
    env.wait_status(&second.id, JobStatus::Failed, WAIT);
    ex.shutdown();
    handle.join().unwrap();
}

#[test]
fn corrupt_result_is_not_saved_and_fails_the_job() {
    let env = new_env();
    let bad = fake_engine(&env.root, r#"{"version":7,"nonsense":true}"#, false);
    let job = env.add_job("битый.mp3", Some(MODEL));
    let ex = env.executor(&bad);
    let handle = ex.spawn();
    let failed = env.wait_status(&job.id, JobStatus::Failed, WAIT);
    assert!(failed.error_code.is_some());
    assert!(env
        .store
        .files
        .read(&job.id, "original.json")
        .unwrap()
        .is_none());
    ex.shutdown();
    handle.join().unwrap();
}

#[test]
fn shutdown_during_processing_marks_the_job_interrupted() {
    let env = new_env();
    let exe = fake_engine(&env.root, RESULT, true);
    let job = env.add_job("закрыли.mp3", Some(MODEL));
    let ex = env.executor(&exe);
    let handle = ex.spawn();
    env.wait_status(&job.id, JobStatus::Processing, WAIT);
    let t0 = Instant::now();
    ex.shutdown();
    handle.join().unwrap();
    assert!(t0.elapsed() < Duration::from_secs(3));
    let after = env.job(&job.id);
    assert_eq!(after.status, JobStatus::Failed);
    assert_eq!(after.error_code.as_deref(), Some("INTERRUPTED"));
    // «Повторить» после перезапуска работает.
    queue::retry(&env.store.db.conn(), &job.id).unwrap();
}

/// Сквозная проверка с настоящим движком и моделями (macOS, нужны собранный `tolk-engine` и модели
/// `small-q5_1` и silero-vad рядом с проектом; иначе тест пропускается).
#[cfg(target_os = "macos")]
#[test]
fn real_engine_completes_a_job_and_cancels_another() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let engine = std::env::var_os("TOLK_ENGINE")
        .map(PathBuf::from)
        .or_else(|| {
            ["release", "debug"]
                .iter()
                .map(|p| root.join("target").join(p).join("tolk-engine"))
                .find(|p| p.is_file())
        });
    let models_src = std::env::var_os("TOLK_TEST_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../dev-tolk-tools/models"));
    let (small, vad) = (
        models_src.join("ggml-small-q5_1.bin"),
        models_src.join("ggml-silero-v6.2.0.bin"),
    );
    let Some(engine) = engine.filter(|_| small.is_file() && vad.is_file()) else {
        eprintln!("ПРОПУЩЕН: нет tolk-engine или моделей");
        return;
    };

    let env = new_env();
    let models = env.root.join("data").join("models");
    for (src, id) in [(&small, MODEL), (&vad, VAD_MODEL_ID)] {
        let dest = models.join(&find(id).unwrap().file);
        fs::remove_file(&dest).unwrap();
        std::os::unix::fs::symlink(src, dest).unwrap();
    }
    let phrase = env.root.join("phrase.aiff");
    let text =
        "Добрый день, коллеги. Сегодня мы обсудим план работ на следующий квартал. ".repeat(6);
    let status = std::process::Command::new("/usr/bin/say")
        .args(["-v", "Milena", "-o"])
        .arg(&phrase)
        .arg(&text)
        .status();
    if !status.is_ok_and(|s| s.success()) {
        eprintln!("ПРОПУЩЕН: нет голоса Milena для say");
        return;
    }

    let job = env.add_job("речь.aiff", Some(MODEL));
    fs::copy(
        &phrase,
        env.store.files.layout().job_dir(&job.id).join("source.wav"),
    )
    .unwrap();
    let ex = env.executor(&engine);
    let handle = ex.spawn();
    env.wait_status(&job.id, JobStatus::Completed, Duration::from_secs(120));
    let bytes = env
        .store
        .files
        .read(&job.id, "original.json")
        .unwrap()
        .unwrap();
    let t = tolk_core::transcript::Transcript::from_json(&bytes).unwrap();
    let all: String = t.segments.iter().map(|s| s.text.to_lowercase()).collect();
    assert!(all.contains("квартал"), "распознано: {all}");
    assert!(env
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, JobEvent::Progress { percent: 100, .. })));

    // Отмена настоящего движка за ≤ 2 с.
    let second = env.add_job("ещё.aiff", Some(MODEL));
    fs::copy(
        &phrase,
        env.store
            .files
            .layout()
            .job_dir(&second.id)
            .join("source.wav"),
    )
    .unwrap();
    env.wait_status(&second.id, JobStatus::Processing, WAIT);
    let t0 = Instant::now();
    ex.cancel_job(&second.id).unwrap();
    // Задание могло успеть завершиться (короткая запись): тогда отмена ничего не меняет.
    let end = {
        let t = Instant::now();
        loop {
            let j = env.job(&second.id);
            if matches!(j.status, JobStatus::Cancelled | JobStatus::Completed) {
                break j;
            }
            assert!(t.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    if end.status == JobStatus::Cancelled {
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "отмена заняла {:?}",
            t0.elapsed()
        );
    }
    ex.shutdown();
    handle.join().unwrap();
}
