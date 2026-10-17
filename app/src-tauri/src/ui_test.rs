//! Автоматический прогон сценария в настоящем окне приложения (feature `ui-test`, в выпуск не входит).
//!
//! Системные средства управления окном (Универсальный доступ, запись экрана) на машине разработчика
//! не выданы, а `tauri-driver` не поддерживает macOS. Поэтому скрипт из `TOLK_UI_TEST_SCRIPT`
//! выполняется внутри страницы, а результаты уходят командой `ui_test_report` в файл
//! `TOLK_UI_TEST_LOG`. Сценарий проходит через настоящий WebView, IPC, сайдкар и закрытие окна.

use std::io::Write;
use std::time::Duration;

use tauri::{AppHandle, Manager};

pub fn start(app: &AppHandle) {
    let Some(script_path) = std::env::var_os("TOLK_UI_TEST_SCRIPT") else {
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        let Ok(script) = std::fs::read_to_string(&script_path) else {
            log::error!("ui-test: не удалось прочитать скрипт");
            return;
        };
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.set_focus();
            let _ = window.eval(script);
        }
    });
}

#[tauri::command]
pub fn ui_test_report(line: String) {
    if let Some(path) = std::env::var_os("TOLK_UI_TEST_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Закрывает окно так же, как кнопка закрытия: через `CloseRequested`.
#[tauri::command]
pub fn ui_test_close(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.close();
    }
}
