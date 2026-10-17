//! Паника в любом потоке попадает в журнал. Отдельный тестовый бинарник: логгер и обработчик паник глобальны.

use std::sync::{Arc, Mutex};

use log::{Level, Log, Metadata, Record};

struct Capture(Arc<Mutex<Vec<(Level, String)>>>);

impl Log for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &Record<'_>) {
        self.0
            .lock()
            .unwrap()
            .push((record.level(), record.args().to_string()));
    }
    fn flush(&self) {}
}

#[test]
fn panic_is_written_to_the_log_with_place_and_message() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let logger: &'static Capture = Box::leak(Box::new(Capture(Arc::clone(&records))));
    log::set_logger(logger).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    tolk_core::diagnostics::install_panic_hook();

    let long_text = "я".repeat(1000);
    let result = std::thread::Builder::new()
        .name("рабочий-поток".into())
        .spawn(move || {
            if long_text.len() > 1 {
                panic!("сбой декодера: {long_text}");
            }
        })
        .unwrap()
        .join();
    assert!(result.is_err());

    let records = records.lock().unwrap();
    let (level, text) = records
        .iter()
        .find(|(_, t)| t.contains("Паника"))
        .expect("паника записана в журнал");
    assert_eq!(*level, Level::Error);
    assert!(text.contains("рабочий-поток"), "имя потока: {text}");
    assert!(text.contains("panic_hook.rs"), "место паники: {text}");
    assert!(text.contains("сбой декодера"));
    assert!(
        text.matches('я').count() <= 300,
        "длинное сообщение обрезано: {}",
        text.matches('я').count()
    );
}
