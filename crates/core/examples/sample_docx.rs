//! Пример: создаёт тестовый DOCX для ручной проверки в Word, LibreOffice, Google Docs, Pages.
//! `cargo run -p tolk-core --example sample_docx -- <файл.docx> [--timestamps]`
use tolk_core::export::{export_to_file, ExportOptions};
use tolk_core::transcript::{Segment, Transcript, FORMAT_VERSION};

fn main() {
    let mut args = std::env::args().skip(1);
    let dest = args.next().expect("укажите путь к .docx");
    let timestamps = args.any(|a| a == "--timestamps");
    let phrases = [
        "Добрый день, коллеги. Сегодня мы обсудим план работ на следующий квартал.",
        "Первым пунктом стоит перенос сервисов в облако, вторым — настройка мониторинга и оповещений.",
        "Ответственным за миграцию назначен Иван Петров. Срок выполнения — пятнадцатое ноября.",
        "Если возникнут вопросы, пишите в общий чат: «Ё-моё», это же Jira и Kubernetes.",
    ];
    let mut segments = Vec::new();
    let mut t = 0u64;
    for i in 0..400u64 {
        let text = phrases[(i % 4) as usize];
        segments.push(Segment {
            start_ms: t,
            end_ms: t + 5_000,
            text: text.into(),
        });
        t += if i % 9 == 8 { 8_000 } else { 5_300 };
    }
    let transcript = Transcript {
        version: FORMAT_VERSION,
        engine: "образец".into(),
        model: "large-v3-turbo-q5_0".into(),
        language: "ru".into(),
        duration_ms: t,
        segments,
    };
    let opts = ExportOptions {
        title: "Совещание по планированию".into(),
        date: "2026-10-18".into(),
        timestamps,
    };
    export_to_file(&transcript, &opts, std::path::Path::new(&dest)).expect("экспорт");
    println!("записано: {dest}");
}
