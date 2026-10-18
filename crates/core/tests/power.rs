#[cfg(target_os = "macos")]
use tolk_core::db::Db;
#[cfg(target_os = "macos")]
use tolk_core::jobs::{self, NewJob};
use tolk_core::power::{SleepGuard, SleepInhibitor};
#[cfg(target_os = "macos")]
use tolk_core::queue;

/// Тесты проверяют запрет сна всего процесса, поэтому выполняются по одному.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// Есть ли в `pmset -g assertions` запрет `PreventUserIdleSystemSleep` от этого процесса.
#[cfg(target_os = "macos")]
fn has_own_assertion() -> bool {
    let out = std::process::Command::new("/usr/bin/pmset")
        .args(["-g", "assertions"])
        .output()
        .expect("pmset");
    let text = String::from_utf8_lossy(&out.stdout);
    let own = format!("pid {}(", std::process::id());
    text.lines()
        .any(|l| l.contains(&own) && l.contains("PreventUserIdleSystemSleep"))
}

/// Проверка настоящего запрета сна через системную утилиту `pmset`: запрет виден, пока `SleepGuard` жив,
/// и пропадает после его уничтожения.
#[cfg(target_os = "macos")]
#[test]
fn macos_assertion_is_visible_in_pmset_while_guard_lives() {
    let _serial = serial();
    assert!(!has_own_assertion(), "до запроса такого запрета нет");
    let guard = SleepGuard::acquire().expect("запрет сна");
    assert!(
        has_own_assertion(),
        "запрет PreventUserIdleSystemSleep виден в pmset"
    );
    drop(guard);
    assert!(!has_own_assertion(), "после снятия запрета не осталось");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_inhibitor_follows_the_queue_and_releases_when_processing_ends() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("tolk.db")).unwrap();
    let job = jobs::insert(
        &db.conn(),
        &NewJob {
            source_name: "a".into(),
            ..NewJob::default()
        },
    )
    .unwrap();
    let mut inhibitor = SleepInhibitor::new();

    inhibitor.sync_with_queue(&db.conn()).unwrap();
    assert!(!inhibitor.is_active(), "очередь без обработки: запрета нет");

    queue::start(&db.conn(), &job.id).unwrap();
    inhibitor.sync_with_queue(&db.conn()).unwrap();
    assert!(inhibitor.is_active());
    assert!(has_own_assertion());
    inhibitor.sync_with_queue(&db.conn()).unwrap(); // повтор безопасен
    assert!(inhibitor.is_active());

    queue::complete(&db.conn(), &job.id).unwrap();
    inhibitor.sync_with_queue(&db.conn()).unwrap();
    assert!(!inhibitor.is_active());
    assert!(
        !has_own_assertion(),
        "после завершения очереди система снова может засыпать"
    );
}

#[test]
fn inhibitor_calls_are_idempotent_and_survive_acquire_failures() {
    let _serial = serial();
    let mut inhibitor = SleepInhibitor::new();
    inhibitor.set_active(false);
    assert!(!inhibitor.is_active());
    inhibitor.set_active(true);
    inhibitor.set_active(true);
    inhibitor.set_active(false);
    inhibitor.set_active(false);
    assert!(!inhibitor.is_active());
}

#[test]
fn dropping_the_inhibitor_releases_the_guard() {
    let _serial = serial();
    let mut inhibitor = SleepInhibitor::new();
    inhibitor.set_active(true);
    drop(inhibitor); // не должно зависнуть и не должно паниковать
    if let Ok(g) = SleepGuard::acquire() {
        drop(g);
    }
}
