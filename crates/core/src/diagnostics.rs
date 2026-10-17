//! Диагностика: паники попадают в журнал (MVP-025).
//!
//! В журнал пишутся место паники и сообщение; тексты транскриптов в журналы не пишутся (правило проекта):
//! сообщения паник формируются из служебных значений, а длина текста ограничена.

use std::backtrace::Backtrace;
use std::panic::{self, PanicHookInfo};

/// Сколько символов сообщения паники попадает в журнал.
const MAX_MESSAGE_CHARS: usize = 300;

fn payload_text(info: &PanicHookInfo<'_>) -> String {
    let raw = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(не текст)".to_string());
    raw.chars().take(MAX_MESSAGE_CHARS).collect()
}

/// Записывает любую панику процесса в журнал (`log::error!`) и затем вызывает прежний обработчик.
/// Вызывать после инициализации журнала.
pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let place = info.location().map_or_else(
            || "неизвестное место".to_string(),
            |l| format!("{}:{}", l.file(), l.line()),
        );
        let thread = std::thread::current();
        log::error!(
            "Паника в потоке «{}» ({place}): {}\n{}",
            thread.name().unwrap_or("без имени"),
            payload_text(info),
            Backtrace::force_capture()
        );
        previous(info);
    }));
}
