use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use tolk_core::db::Db;
use tolk_core::jobs::{self, Job, JobStatus, NewJob};
use tolk_core::layout::DataLayout;
use tolk_core::queue::{self, can_transition, ProgressThrottle};
use tolk_core::store::Store;
use tolk_core::StoreError;

use JobStatus::*;

const ALL: [JobStatus; 5] = [Queued, Processing, Completed, Failed, Cancelled];

fn open() -> (TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("tolk.db")).unwrap();
    (dir, db)
}

fn add(db: &Db, name: &str) -> Job {
    jobs::insert(
        &db.conn(),
        &NewJob {
            source_name: name.into(),
            ..NewJob::default()
        },
    )
    .unwrap()
}

fn status(db: &Db, id: &str) -> JobStatus {
    jobs::get(&db.conn(), id).unwrap().unwrap().status
}

fn is_invalid(err: StoreError, from: &str) -> bool {
    matches!(err, StoreError::InvalidTransition { from: f, .. } if f == from)
}

#[test]
fn transition_table_allows_exactly_the_documented_moves() {
    let allowed = [
        (Queued, Processing),
        (Queued, Cancelled),
        (Processing, Completed),
        (Processing, Failed),
        (Processing, Cancelled),
        (Failed, Queued),
        (Cancelled, Queued),
    ];
    for from in ALL {
        for to in ALL {
            assert_eq!(
                can_transition(from, to),
                allowed.contains(&(from, to)),
                "{from:?} -> {to:?}"
            );
        }
    }
    for to in ALL {
        assert!(
            !can_transition(Completed, to),
            "COMPLETED — конечное состояние"
        );
    }
}

/// Каждое запрещённое таблицей действие над заданием в данном статусе отклоняется базой и ничего не меняет.
#[test]
fn database_enforces_the_same_table_for_every_operation() {
    let (_d, db) = open();
    let conn = db.conn();
    let reach = |target: JobStatus| -> String {
        let job = jobs::insert(
            &conn,
            &NewJob {
                source_name: "x".into(),
                ..NewJob::default()
            },
        )
        .unwrap();
        // Кроме проверяемого задания, других PROCESSING быть не должно.
        conn.execute(
            "UPDATE jobs SET status = 'CANCELLED' WHERE status = 'PROCESSING'",
            [],
        )
        .unwrap();
        match target {
            Queued => {}
            Processing => {
                queue::start(&conn, &job.id).unwrap();
            }
            Completed => {
                queue::start(&conn, &job.id).unwrap();
                queue::complete(&conn, &job.id).unwrap();
            }
            Failed => {
                queue::start(&conn, &job.id).unwrap();
                queue::fail(&conn, &job.id, "E", "m").unwrap();
            }
            Cancelled => {
                queue::cancel(&conn, &job.id).unwrap();
            }
        }
        job.id
    };
    type Op = fn(&rusqlite::Connection, &str) -> tolk_core::Result<Job>;
    let ops: [(&str, JobStatus, Op); 5] = [
        ("start", Processing, queue::start),
        ("complete", Completed, queue::complete),
        ("fail", Failed, |c, id| queue::fail(c, id, "E", "m")),
        ("cancel", Cancelled, queue::cancel),
        ("retry", Queued, queue::retry),
    ];
    for (name, to, op) in ops {
        for from in ALL {
            let id = reach(from);
            let before = jobs::get(&conn, &id).unwrap().unwrap();
            let result = op(&conn, &id);
            let after = jobs::get(&conn, &id).unwrap().unwrap();
            if can_transition(from, to) {
                assert_eq!(result.unwrap().status, to, "{name} из {from:?}");
            } else {
                assert!(
                    is_invalid(result.unwrap_err(), from.as_str()),
                    "{name} из {from:?}"
                );
                assert_eq!(before, after, "{name} из {from:?} не должен ничего менять");
            }
        }
    }
}

#[test]
fn unknown_job_gives_not_found() {
    let (_d, db) = open();
    assert!(matches!(
        queue::start(&db.conn(), "нет"),
        Err(StoreError::JobNotFound(_))
    ));
    assert!(matches!(
        queue::cancel(&db.conn(), "нет"),
        Err(StoreError::JobNotFound(_))
    ));
}

#[test]
fn claim_is_fifo_and_only_one_job_runs_at_a_time() {
    let (_d, db) = open();
    let a = add(&db, "a");
    let b = add(&db, "b");
    let first = queue::claim_next(&db.conn()).unwrap().expect("задание");
    assert_eq!(first.id, a.id);
    assert_eq!(
        (first.status, first.attempts, first.progress),
        (Processing, 1, 0)
    );
    assert!(first.started_at.is_some());

    assert!(
        queue::claim_next(&db.conn()).unwrap().is_none(),
        "пока идёт a, b не берётся"
    );
    assert!(is_invalid(
        queue::start(&db.conn(), &b.id).unwrap_err(),
        "QUEUED"
    ));
    assert_eq!(status(&db, &b.id), Queued);

    queue::complete(&db.conn(), &a.id).unwrap();
    assert_eq!(queue::claim_next(&db.conn()).unwrap().unwrap().id, b.id);
}

#[test]
fn double_click_on_start_and_retry_does_not_start_twice() {
    let (_d, db) = open();
    let job = add(&db, "a");
    queue::start(&db.conn(), &job.id).unwrap();
    assert!(is_invalid(
        queue::start(&db.conn(), &job.id).unwrap_err(),
        "PROCESSING"
    ));
    assert_eq!(jobs::get(&db.conn(), &job.id).unwrap().unwrap().attempts, 1);

    queue::fail(&db.conn(), &job.id, "ENGINE", "сбой").unwrap();
    queue::retry(&db.conn(), &job.id).unwrap();
    assert!(
        is_invalid(queue::retry(&db.conn(), &job.id).unwrap_err(), "QUEUED"),
        "второй клик «Повторить»"
    );
}

#[test]
fn concurrent_claims_start_exactly_one_job() {
    let (_d, db) = open();
    for i in 0..6 {
        add(&db, &format!("{i}"));
    }
    let db = Arc::new(db);
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (db, barrier) = (Arc::clone(&db), Arc::clone(&barrier));
            thread::spawn(move || {
                barrier.wait();
                let claimed = queue::claim_next(&db.conn()).unwrap();
                claimed.is_some()
            })
        })
        .collect();
    let started = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|&s| s)
        .count();
    assert_eq!(started, 1);
    let processing: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE status = 'PROCESSING'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(processing, 1);
}

#[test]
fn queue_of_six_runs_sequentially_in_order() {
    let (_d, db) = open();
    let ids: Vec<String> = (0..6)
        .map(|i| add(&db, &format!("запись {i}")).id)
        .collect();
    let mut done = Vec::new();
    loop {
        // Guard соединения не должен жить в теле цикла: `while let` держал бы его до конца итерации.
        let claimed = queue::claim_next(&db.conn()).unwrap();
        let Some(job) = claimed else { break };
        let running: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE status = 'PROCESSING'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(running, 1);
        assert!(queue::set_progress(&db.conn(), &job.id, 50).unwrap());
        queue::complete(&db.conn(), &job.id).unwrap();
        done.push(job.id);
    }
    assert_eq!(done, ids);
    for id in ids {
        let j = jobs::get(&db.conn(), &id).unwrap().unwrap();
        assert_eq!((j.status, j.progress), (Completed, 100));
        assert!(j.finished_at.is_some());
    }
}

#[test]
fn cancelling_a_queued_job_prevents_it_from_starting() {
    let (_d, db) = open();
    let a = add(&db, "a");
    let b = add(&db, "b");
    queue::cancel(&db.conn(), &a.id).unwrap();
    assert_eq!(status(&db, &a.id), Cancelled);
    assert_eq!(queue::claim_next(&db.conn()).unwrap().unwrap().id, b.id);
    assert!(is_invalid(
        queue::start(&db.conn(), &a.id).unwrap_err(),
        "CANCELLED"
    ));
}

#[test]
fn cancel_and_complete_race_has_exactly_one_winner() {
    let (_d, db) = open();
    let job = add(&db, "a");
    queue::start(&db.conn(), &job.id).unwrap();
    // Завершение успело раньше отмены: отмена отклоняется, результат не теряется.
    queue::complete(&db.conn(), &job.id).unwrap();
    assert!(is_invalid(
        queue::cancel(&db.conn(), &job.id).unwrap_err(),
        "COMPLETED"
    ));
    assert_eq!(status(&db, &job.id), Completed);

    // Отмена успела раньше завершения: завершение отклоняется.
    let job2 = add(&db, "b");
    queue::start(&db.conn(), &job2.id).unwrap();
    queue::cancel(&db.conn(), &job2.id).unwrap();
    assert!(is_invalid(
        queue::complete(&db.conn(), &job2.id).unwrap_err(),
        "CANCELLED"
    ));
    assert!(is_invalid(
        queue::fail(&db.conn(), &job2.id, "E", "m").unwrap_err(),
        "CANCELLED"
    ));
    assert_eq!(status(&db, &job2.id), Cancelled);
}

#[test]
fn failure_keeps_error_and_retry_resets_it_but_keeps_attempts() {
    let (_d, db) = open();
    let job = add(&db, "a");
    queue::start(&db.conn(), &job.id).unwrap();
    queue::set_progress(&db.conn(), &job.id, 40).unwrap();
    let failed = queue::fail(
        &db.conn(),
        &job.id,
        "ENGINE_CRASH",
        "Движок завершился с ошибкой",
    )
    .unwrap();
    assert_eq!(
        (failed.error_code.as_deref(), failed.progress),
        (Some("ENGINE_CRASH"), 40)
    );

    let again = queue::retry(&db.conn(), &job.id).unwrap();
    assert_eq!(
        (again.status, again.progress, again.attempts),
        (Queued, 0, 1)
    );
    assert!(
        again.error_code.is_none() && again.error_message.is_none() && again.finished_at.is_none()
    );

    let second = queue::claim_next(&db.conn()).unwrap().unwrap();
    assert_eq!(second.attempts, 2);
}

#[test]
fn progress_is_written_only_for_a_running_job_and_clamped() {
    let (_d, db) = open();
    let job = add(&db, "a");
    assert!(
        !queue::set_progress(&db.conn(), &job.id, 10).unwrap(),
        "QUEUED прогресс не принимает"
    );
    queue::start(&db.conn(), &job.id).unwrap();
    assert!(queue::set_progress(&db.conn(), &job.id, 250).unwrap());
    assert_eq!(
        jobs::get(&db.conn(), &job.id).unwrap().unwrap().progress,
        100
    );
    queue::complete(&db.conn(), &job.id).unwrap();
    assert!(
        !queue::set_progress(&db.conn(), &job.id, 5).unwrap(),
        "после завершения прогресс не меняется"
    );
}

#[test]
fn recovery_marks_interrupted_jobs_failed_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let layout = DataLayout::new(dir.path().join("data"));
    let (store, first) = Store::open(layout.clone()).unwrap();
    assert_eq!(first.interrupted, 0);
    let running = add(&store.db, "идёт");
    let waiting = add(&store.db, "ждёт");
    let finished = add(&store.db, "готово");
    queue::start(&store.db.conn(), &finished.id).unwrap();
    queue::complete(&store.db.conn(), &finished.id).unwrap();
    queue::start(&store.db.conn(), &running.id).unwrap();
    queue::set_progress(&store.db.conn(), &running.id, 63).unwrap();
    drop(store); // «принудительное завершение»: статус PROCESSING остался в базе

    let (store, cleanup) = Store::open(layout.clone()).unwrap();
    assert_eq!(cleanup.interrupted, 1);
    let j = jobs::get(&store.db.conn(), &running.id).unwrap().unwrap();
    assert_eq!(j.status, Failed);
    assert_eq!(
        (j.error_code.as_deref(), j.error_message.as_deref()),
        (Some("INTERRUPTED"), Some("Обработка прервана"))
    );
    assert_eq!(
        status(&store.db, &waiting.id),
        Queued,
        "ожидающее задание не тронуто"
    );
    assert_eq!(
        status(&store.db, &finished.id),
        Completed,
        "готовое задание не тронуто"
    );

    // Автозапуска нет: задание ждёт, пока пользователь не нажмёт «Повторить».
    assert!(queue::claim_next(&store.db.conn()).unwrap().map(|j| j.id) == Some(waiting.id.clone()));
    queue::complete(&store.db.conn(), &waiting.id).unwrap();
    assert!(queue::claim_next(&store.db.conn()).unwrap().is_none());
    queue::retry(&store.db.conn(), &running.id).unwrap();
    assert_eq!(
        queue::claim_next(&store.db.conn()).unwrap().unwrap().id,
        running.id
    );
    drop(store);

    // Повторное открытие идемпотентно: PROCESSING снова станет FAILED, остальное не меняется.
    let (_s, again) = Store::open(layout).unwrap();
    assert_eq!(again.interrupted, 1);
}

#[test]
fn progress_throttle_limits_writes_but_never_drops_completion() {
    let mut t = ProgressThrottle::new(Duration::from_secs(3));
    let t0 = Instant::now();
    assert!(t.should_write(t0, 1), "первая запись проходит");
    assert!(
        !t.should_write(t0 + Duration::from_secs(1), 2),
        "слишком рано"
    );
    assert!(!t.should_write(t0 + Duration::from_secs(2), 3));
    assert!(
        t.should_write(t0 + Duration::from_secs(3), 4),
        "интервал прошёл"
    );
    assert!(
        !t.should_write(t0 + Duration::from_secs(4), 4),
        "то же значение не пишется"
    );
    assert!(
        t.should_write(t0 + Duration::from_secs(4), 100),
        "100% пишется сразу"
    );
}
