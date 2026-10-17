//! Исполнитель очереди: берёт задания по одному, запускает движок, сохраняет результат и переводит
//! задание в конечный статус (MVP-016, TD-10).
//!
//! Один рабочий поток. Всё, что решается «кто успел первым» (захват, отмена, завершение), решается
//! условными `UPDATE` в [`queue`](crate::queue); здесь дополнительно гарантируется, что отмена не
//! вклинится между захватом задания и регистрацией его токена отмены (оба шага под одной блокировкой).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::download::{is_installed, model_path, CancelToken};
use crate::engine::{self, EngineEvent, RunResult, TranscribeArgs};
use crate::error::{Result, StoreError};
use crate::jobs::{self, Job, JobStatus};
use crate::metrics::{self, JobMetrics, Outcome};
use crate::model_manager;
use crate::models::{find, VAD_MODEL_ID};
use crate::power::SleepInhibitor;
use crate::queue::{self, ProgressThrottle, ERROR_INTERRUPTED};
use crate::store::Store;
use crate::transcript::Transcript;
use crate::user_errors;

pub const DEFAULT_LANGUAGE: &str = "ru";
/// Как часто прогресс записывается в базу.
const DB_PROGRESS_INTERVAL: Duration = Duration::from_secs(3);
/// Как долго рабочий поток ждёт сигнала, прежде чем всё равно заглянуть в очередь.
const IDLE_WAIT: Duration = Duration::from_secs(2);
const ORIGINAL_FILE: &str = "original.json";
const TRANSCRIPT_FILE: &str = "transcript.json";

/// События для интерфейса. Текст распознанного сюда не попадает.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JobEvent {
    #[serde(rename_all = "camelCase")]
    Started { job_id: String },
    #[serde(rename_all = "camelCase")]
    Progress { job_id: String, percent: u8 },
    /// Задание перешло в конечный статус (`COMPLETED`, `FAILED`, `CANCELLED`).
    #[serde(rename_all = "camelCase")]
    Finished {
        job_id: String,
        status: String,
        error_code: Option<String>,
        error_message: Option<String>,
    },
}

pub type EventSink = Arc<dyn Fn(JobEvent) + Send + Sync>;

pub struct Executor {
    store: Arc<Store>,
    engine_exe: PathBuf,
    inhibitor: Arc<Mutex<SleepInhibitor>>,
    on_event: EventSink,
    /// Идущее задание и его токен отмены. Блокировка берётся и при захвате, и при отмене.
    current: Mutex<Option<(String, CancelToken)>>,
    stop: AtomicBool,
    wake: (Mutex<bool>, Condvar),
}

impl Executor {
    pub fn new(
        store: Arc<Store>,
        engine_exe: PathBuf,
        inhibitor: Arc<Mutex<SleepInhibitor>>,
        on_event: EventSink,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            engine_exe,
            inhibitor,
            on_event,
            current: Mutex::new(None),
            stop: AtomicBool::new(false),
            wake: (Mutex::new(false), Condvar::new()),
        })
    }

    /// Запускает рабочий поток.
    pub fn spawn(self: &Arc<Self>) -> JoinHandle<()> {
        let me = Arc::clone(self);
        thread::Builder::new()
            .name("tolk-queue".into())
            .spawn(move || me.run_loop())
            .expect("не удалось запустить поток очереди")
    }

    /// Сообщает, что в очереди появилось задание (добавление, повтор).
    pub fn notify(&self) {
        *self.wake.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.1.notify_all();
    }

    /// Останавливает исполнитель: идущее задание прерывается и помечается «Обработка прервана»
    /// (как при падении приложения), новые не берутся.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some((_, token)) = self
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            token.cancel();
        }
        self.notify();
    }

    /// Ждёт, пока идущее задание не будет записано в базу (после `shutdown`). `false` — не дождались.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < timeout {
            if self
                .current
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_none()
            {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// Отмена. Задание в очереди отменяется сразу; идущее — завершением процесса движка (статус
    /// `CANCELLED` выставит рабочий поток после его остановки).
    pub fn cancel_job(&self, job_id: &str) -> Result<Job> {
        let current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((id, token)) = current.as_ref() {
            if id == job_id {
                token.cancel();
                return jobs::get(&self.store.db.conn(), job_id)?
                    .ok_or_else(|| StoreError::JobNotFound(job_id.to_string()));
            }
        }
        let conn = self.store.db.conn();
        queue::cancel(&conn, job_id)
    }

    /// Задание обрабатывается прямо сейчас этим исполнителем.
    pub fn is_running(&self, job_id: &str) -> bool {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|(id, _)| id == job_id)
    }

    fn run_loop(&self) {
        while !self.stop.load(Ordering::SeqCst) {
            match self.claim() {
                Ok(Some((job, token))) => self.process(&job, &token),
                Ok(None) => self.wait_for_work(),
                Err(e) => {
                    log::error!("Очередь: ошибка базы данных: {e}");
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    fn wait_for_work(&self) {
        let (lock, cv) = &self.wake;
        let mut flag = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !*flag {
            flag = cv
                .wait_timeout(flag, IDLE_WAIT)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *flag = false;
    }

    /// Захват задания и регистрация токена отмены — под одной блокировкой.
    fn claim(&self) -> Result<Option<(Job, CancelToken)>> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let job = {
            let conn = self.store.db.conn();
            queue::claim_next(&conn)?
        };
        let Some(job) = job else { return Ok(None) };
        let token = CancelToken::new();
        *current = Some((job.id.clone(), token.clone()));
        Ok(Some((job, token)))
    }

    fn sync_sleep_inhibitor(&self) {
        let conn = self.store.db.conn();
        if let Err(e) = self
            .inhibitor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sync_with_queue(&conn)
        {
            log::warn!("Не удалось обновить запрет сна: {e}");
        }
    }

    fn process(&self, job: &Job, token: &CancelToken) {
        log::info!(
            "Очередь: задание {} взято в работу (попытка {})",
            job.id,
            job.attempts
        );
        self.sync_sleep_inhibitor();
        (self.on_event)(JobEvent::Started {
            job_id: job.id.clone(),
        });
        let started = Instant::now();
        let outcome = self.run_job(job, token);
        self.finish(job, outcome, started);
    }

    /// Запускает движок и сохраняет результат. Возвращает итог для записи в базу.
    fn run_job(&self, job: &Job, token: &CancelToken) -> Finish {
        let layout = self.store.files.layout();
        let models_dir = layout.models_dir();

        let model_id = match job.model_id.clone() {
            Some(id) => Some(id),
            None => model_manager::model_for_new_job(&self.store.db, &models_dir)
                .ok()
                .flatten(),
        };
        let entry = model_id.as_deref().and_then(find);
        let vad = find(VAD_MODEL_ID);
        let (Some(entry), Some(vad)) = (entry, vad) else {
            return Finish::engine_error("MODEL_MISSING", None, model_id);
        };
        if !is_installed(entry, &models_dir) || !is_installed(vad, &models_dir) {
            return Finish::engine_error("MODEL_MISSING", None, Some(entry.id.clone()));
        }
        let model_id = Some(entry.id.clone());

        let Some(audio_name) = job.audio_path.as_deref() else {
            return Finish::engine_error("FILE_NOT_FOUND", None, model_id);
        };
        let input = layout.job_dir(&job.id).join(audio_name);
        if !input.is_file() {
            return Finish::engine_error("FILE_NOT_FOUND", None, model_id);
        }

        let output = layout.tmp_dir().join(format!("{}.original.json", job.id));
        let args = TranscribeArgs {
            model: model_path(entry, &models_dir),
            vad_model: Some(model_path(vad, &models_dir)),
            input,
            output: output.clone(),
            language: DEFAULT_LANGUAGE.into(),
            threads: None,
            no_gpu: false,
        };
        let log_path = engine::engine_log_path(layout);
        let mut throttle = ProgressThrottle::new(DB_PROGRESS_INTERVAL);
        let result = engine::run_transcribe(
            &self.engine_exe,
            &args,
            Some(&log_path),
            token,
            &mut |event| {
                if let EngineEvent::Progress { percent } = event {
                    if throttle.should_write(Instant::now(), *percent) {
                        let _ = queue::set_progress(&self.store.db.conn(), &job.id, *percent);
                    }
                    (self.on_event)(JobEvent::Progress {
                        job_id: job.id.clone(),
                        percent: *percent,
                    });
                }
            },
        );
        let finish = match result {
            Err(e) => {
                log::error!("Очередь: не удалось запустить движок: {e}");
                Finish::engine_error("INTERNAL", None, model_id.clone())
            }
            Ok(RunResult::Done(done)) => match self.store_result(&job.id, &output) {
                Ok(()) => Finish::Completed {
                    audio_ms: done.audio_ms,
                    peak_rss_bytes: done.peak_rss_bytes,
                    backend: Some(done.backend),
                    model_id: model_id.clone(),
                },
                Err(e) => {
                    log::error!("Очередь: не удалось сохранить результат: {e}");
                    let m = user_errors::from_store(&e);
                    Finish::Failed {
                        code: m.code,
                        message: m.message,
                        model_id: model_id.clone(),
                    }
                }
            },
            Ok(RunResult::Failed(e)) => {
                Finish::engine_error(&e.code, e.codec.as_deref(), model_id.clone())
            }
            Ok(RunResult::Cancelled) => Finish::Cancelled {
                model_id: model_id.clone(),
            },
            Ok(RunResult::Crashed {
                exit_code,
                stderr_tail,
            }) => {
                log::error!("Очередь: движок завершился аварийно (код {exit_code:?}); хвост stderr: {stderr_tail}");
                Finish::engine_error("INTERNAL", None, model_id.clone())
            }
        };
        let _ = std::fs::remove_file(&output);
        finish
    }

    /// Проверяет результат движка и кладёт его в каталог задания: `original.json` неизменяем,
    /// `transcript.json` — рабочая копия (в MVP-020 её правит пользователь).
    fn store_result(&self, job_id: &str, output: &std::path::Path) -> Result<()> {
        let bytes = std::fs::read(output)?;
        Transcript::from_json(&bytes)?;
        self.store
            .files
            .write_atomic(job_id, ORIGINAL_FILE, &bytes)?;
        self.store
            .files
            .write_atomic(job_id, TRANSCRIPT_FILE, &bytes)?;
        Ok(())
    }

    fn finish(&self, job: &Job, outcome: Finish, started: Instant) {
        let shutting_down = self.stop.load(Ordering::SeqCst);
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let (status, code, message, metrics) = match outcome {
            Finish::Completed {
                audio_ms,
                peak_rss_bytes,
                backend,
                model_id,
            } => (
                JobStatus::Completed,
                None,
                None,
                JobMetrics {
                    audio_ms,
                    processing_ms: elapsed_ms,
                    peak_rss_bytes,
                    backend,
                    model_id,
                    outcome: Outcome::Success,
                    error_code: None,
                },
            ),
            Finish::Failed {
                code,
                message,
                model_id,
            } => {
                let m = failed_metrics(
                    job,
                    elapsed_ms,
                    model_id,
                    Outcome::Failed,
                    Some(code.clone()),
                );
                (JobStatus::Failed, Some(code), Some(message), m)
            }
            // Закрытие приложения прерывает работу так же, как падение: «Обработка прервана».
            Finish::Cancelled { model_id } if shutting_down => {
                let m = failed_metrics(
                    job,
                    elapsed_ms,
                    model_id,
                    Outcome::Cancelled,
                    Some(ERROR_INTERRUPTED.into()),
                );
                (
                    JobStatus::Failed,
                    Some(ERROR_INTERRUPTED.to_string()),
                    Some(queue::MESSAGE_INTERRUPTED.to_string()),
                    m,
                )
            }
            Finish::Cancelled { model_id } => {
                let m = failed_metrics(job, elapsed_ms, model_id, Outcome::Cancelled, None);
                (JobStatus::Cancelled, None, None, m)
            }
        };

        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let result = {
            let conn = self.store.db.conn();
            let r = match status {
                JobStatus::Completed => queue::complete(&conn, &job.id),
                JobStatus::Cancelled => queue::cancel(&conn, &job.id),
                _ => queue::fail(
                    &conn,
                    &job.id,
                    code.as_deref().unwrap_or("INTERNAL"),
                    message.as_deref().unwrap_or(""),
                ),
            };
            if r.is_ok() {
                if let Err(e) = metrics::record(&conn, &job.id, &metrics) {
                    log::warn!("Не удалось сохранить метрики задания: {e}");
                }
            }
            r
        };
        *current = None;
        drop(current);

        if let Err(e) = result {
            log::error!("Очередь: не удалось записать итог задания {}: {e}", job.id);
        }
        log::info!(
            "Очередь: задание {} завершено, статус {}",
            job.id,
            status.as_str()
        );
        self.sync_sleep_inhibitor();
        (self.on_event)(JobEvent::Finished {
            job_id: job.id.clone(),
            status: status.as_str().to_string(),
            error_code: code,
            error_message: message,
        });
    }
}

fn failed_metrics(
    job: &Job,
    elapsed_ms: u64,
    model_id: Option<String>,
    outcome: Outcome,
    code: Option<String>,
) -> JobMetrics {
    JobMetrics {
        audio_ms: job.duration_ms.unwrap_or(0).max(0) as u64,
        processing_ms: elapsed_ms,
        peak_rss_bytes: None,
        backend: None,
        model_id,
        outcome,
        error_code: code,
    }
}

/// Итог обработки до записи в базу.
enum Finish {
    Completed {
        audio_ms: u64,
        peak_rss_bytes: Option<u64>,
        backend: Option<String>,
        model_id: Option<String>,
    },
    Failed {
        code: String,
        message: String,
        model_id: Option<String>,
    },
    Cancelled {
        model_id: Option<String>,
    },
}

impl Finish {
    fn engine_error(code: &str, codec: Option<&str>, model_id: Option<String>) -> Self {
        let m = user_errors::from_engine(code, codec);
        if let Some(t) = &m.technical {
            log::warn!("Очередь: {t}");
        }
        Finish::Failed {
            code: m.code,
            message: m.message,
            model_id,
        }
    }
}
