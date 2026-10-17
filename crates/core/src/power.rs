//! Запрет сна системы во время обработки (TD-10, MVP-017): macOS — `IOPMAssertion`
//! (`PreventUserIdleSystemSleep`), Windows — `SetThreadExecutionState(ES_SYSTEM_REQUIRED)` (крейт `keepawake`).
//!
//! `SetThreadExecutionState` действует на вызвавший поток, а снимается только из него же, поэтому запрет
//! держит отдельный долгоживущий поток: он создаёт запрет и снимает его, когда `SleepGuard` отпускают.
//! Запрет касается только простоя системы: закрытая крышка ноутбука всё равно усыпит компьютер,
//! это отражается в инструкции для сотрудников (MVP-033). Экран гаснуть может.

use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rusqlite::Connection;

use crate::error::Result;

const APP_NAME: &str = "dev-tolk";
const APP_REVERSE_DOMAIN: &str = "kg.devcats.tolk";
/// Причина запрета. `keepawake` 0.6 на macOS имя запрета в `pmset` не выставляет (оно пустое); на Windows причины нет вовсе.
pub const REASON: &str = "dev-tolk: идёт обработка записи";

/// Запрет сна: действует, пока объект жив.
pub struct SleepGuard {
    release: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl SleepGuard {
    /// Запрещает системе уходить в сон из-за простоя. Ошибка возвращается текстом для журнала;
    /// обработка при этом не должна прерываться, поэтому вызывающий код её только логирует.
    pub fn acquire() -> std::result::Result<Self, String> {
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<(), String>>();
        let thread = thread::Builder::new()
            .name("sleep-guard".into())
            .spawn(move || {
                let created = keepawake::Builder::default()
                    .idle(true)
                    .reason(REASON)
                    .app_name(APP_NAME)
                    .app_reverse_domain(APP_REVERSE_DOMAIN)
                    .create();
                match created {
                    Ok(awake) => {
                        let _ = ready_tx.send(Ok(()));
                        let _ = release_rx.recv(); // ждём снятия запрета (или уничтожения хозяина)
                        drop(awake);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                release: Some(release_tx),
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("не удалось получить ответ от потока запрета сна".to_string()),
        }
    }
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        drop(self.release.take()); // закрытие канала будит поток
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Держит запрет сна ровно тогда, когда есть обрабатываемое задание. Повторные вызовы безопасны.
#[derive(Default)]
pub struct SleepInhibitor {
    guard: Option<SleepGuard>,
}

impl SleepInhibitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.guard.is_some()
    }

    /// Включает или выключает запрет. Если система отказала, запись в журнал и работа без запрета.
    pub fn set_active(&mut self, active: bool) {
        match (active, self.guard.is_some()) {
            (true, false) => match SleepGuard::acquire() {
                Ok(guard) => {
                    log::info!("Запрет сна включён: идёт обработка");
                    self.guard = Some(guard);
                }
                Err(e) => log::warn!("Не удалось запретить сон системы: {e}"),
            },
            (false, true) => {
                self.guard = None;
                log::info!("Запрет сна снят");
            }
            _ => {}
        }
    }

    /// Приводит запрет в соответствие с очередью: активен, пока есть задание в PROCESSING.
    /// Вызывать после каждого перехода статуса.
    pub fn sync_with_queue(&mut self, conn: &Connection) -> Result<()> {
        let processing: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE status = 'PROCESSING'",
            [],
            |r| r.get(0),
        )?;
        self.set_active(processing > 0);
        Ok(())
    }
}
