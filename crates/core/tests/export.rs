use std::io::{Cursor, Read};

use tolk_core::export::{
    build_docx, clock, default_file_name, export_to_file, group_paragraphs, ExportOptions,
    PARAGRAPH_MAX_CHARS,
};
use tolk_core::transcript::{Segment, Transcript, FORMAT_VERSION};

fn seg(start_ms: u64, end_ms: u64, text: &str) -> Segment {
    Segment {
        start_ms,
        end_ms,
        text: text.into(),
    }
}

fn transcript(segments: Vec<Segment>) -> Transcript {
    Transcript {
        version: FORMAT_VERSION,
        engine: "whisper.cpp 1.9.5".into(),
        model: "large-v3-turbo-q5_0".into(),
        language: "ru".into(),
        duration_ms: segments.last().map_or(0, |s| s.end_ms),
        segments,
    }
}

fn opts(timestamps: bool) -> ExportOptions {
    ExportOptions {
        title: "Совещание 1".into(),
        date: "2026-10-18".into(),
        timestamps,
    }
}

fn sample() -> Transcript {
    transcript(vec![
        seg(0, 4_000, " Добрый день, коллеги. "),
        seg(4_200, 9_000, "Сегодня обсудим план."),
        seg(15_000, 19_000, "Второй абзац после паузы."),
        seg(3_725_000, 3_730_000, "Час два минуты."),
    ])
}

fn part(docx: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(docx)).expect("docx — это zip");
    let mut s = String::new();
    zip.by_name(name)
        .unwrap_or_else(|_| panic!("в docx нет {name}"))
        .read_to_string(&mut s)
        .unwrap();
    s
}

/// Весь видимый текст документа по абзацам (w:p -> объединённые w:t).
fn paragraphs_text(document_xml: &str) -> Vec<String> {
    let doc = roxmltree::Document::parse(document_xml).expect("document.xml — корректный XML");
    doc.descendants()
        .filter(|n| {
            n.tag_name().name() == "p" && n.ancestors().all(|a| a.tag_name().name() != "footer")
        })
        .map(|p| {
            p.descendants()
                .filter(|n| n.tag_name().name() == "t")
                .filter_map(|n| n.text())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn paragraphs_split_on_pauses_and_normalize_whitespace() {
    let blocks = group_paragraphs(&sample());
    assert_eq!(blocks.len(), 3);
    assert_eq!(
        blocks[0].text,
        "Добрый день, коллеги. Сегодня обсудим план."
    );
    assert_eq!(blocks[0].start_ms, 0);
    assert_eq!(blocks[1].start_ms, 15_000);
    assert_eq!(blocks[2].start_ms, 3_725_000);
}

#[test]
fn pause_threshold_is_two_seconds() {
    let t = transcript(vec![
        seg(0, 1_000, "один"),
        seg(2_999, 3_500, "два"),
        seg(5_500, 6_000, "три"),
    ]);
    let texts: Vec<_> = group_paragraphs(&t).into_iter().map(|b| b.text).collect();
    assert_eq!(
        texts,
        vec!["один два", "три"],
        "пауза 1999 мс не делит, 2000 мс делит"
    );
}

#[test]
fn long_run_without_pauses_is_split_at_segment_boundary() {
    let sentence = "Слово ".repeat(40); // 240 символов
    let segments: Vec<_> = (0..20)
        .map(|i| seg(i * 1_000, i * 1_000 + 900, &sentence))
        .collect();
    let blocks = group_paragraphs(&transcript(segments));
    assert!(blocks.len() > 1);
    for b in &blocks[..blocks.len() - 1] {
        assert!(b.text.chars().count() >= PARAGRAPH_MAX_CHARS);
        assert!(
            b.text.chars().count() < PARAGRAPH_MAX_CHARS + 300,
            "закрывается на ближайшей границе сегмента"
        );
    }
}

#[test]
fn empty_segments_are_skipped_and_empty_transcript_is_valid() {
    let t = transcript(vec![
        seg(0, 1_000, "   "),
        seg(1_000, 2_000, ""),
        seg(2_000, 3_000, "текст"),
    ]);
    let blocks = group_paragraphs(&t);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].start_ms, 2_000);

    let docx = build_docx(&transcript(vec![]), &opts(true)).unwrap();
    assert_eq!(
        paragraphs_text(&part(&docx, "word/document.xml")).len(),
        2,
        "заголовок и метаданные"
    );
}

#[test]
fn clock_format() {
    assert_eq!(clock(0), "00:00:00");
    assert_eq!(clock(59_999), "00:00:59");
    assert_eq!(clock(3_725_000), "01:02:05");
    assert_eq!(clock(100 * 3600 * 1000), "100:00:00");
}

#[test]
fn docx_is_valid_zip_with_expected_parts_and_text() {
    let docx = build_docx(&sample(), &opts(false)).unwrap();
    for name in [
        "[Content_Types].xml",
        "_rels/.rels",
        "word/document.xml",
        "word/styles.xml",
        "word/footer1.xml",
    ] {
        let xml = part(&docx, name);
        roxmltree::Document::parse(&xml).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    let paras = paragraphs_text(&part(&docx, "word/document.xml"));
    assert_eq!(paras[0], "Совещание 1");
    assert_eq!(
        paras[1],
        "Дата: 2026-10-18 · Длительность: 01:02:10 · Модель: large-v3-turbo-q5_0"
    );
    assert_eq!(paras[2], "Добрый день, коллеги. Сегодня обсудим план.");
    assert_eq!(paras[3], "Второй абзац после паузы.");
    assert_eq!(paras[4], "Час два минуты.");
    assert_eq!(paras.len(), 5);
}

#[test]
fn timestamps_are_optional() {
    let with = paragraphs_text(&part(
        &build_docx(&sample(), &opts(true)).unwrap(),
        "word/document.xml",
    ));
    assert_eq!(
        with[2],
        "[00:00:00] Добрый день, коллеги. Сегодня обсудим план."
    );
    assert_eq!(with[3], "[00:00:15] Второй абзац после паузы.");
    assert_eq!(with[4], "[01:02:05] Час два минуты.");

    let without = paragraphs_text(&part(
        &build_docx(&sample(), &opts(false)).unwrap(),
        "word/document.xml",
    ));
    assert!(without.iter().all(|p| !p.contains('[')), "{without:?}");
}

#[test]
fn font_is_explicit_for_every_script_on_every_run() {
    let docx = build_docx(&sample(), &opts(true)).unwrap();
    let xml = part(&docx, "word/document.xml");
    let doc = roxmltree::Document::parse(&xml).unwrap();
    let runs: Vec<_> = doc
        .descendants()
        .filter(|n| n.tag_name().name() == "r")
        .collect();
    assert!(runs.len() >= 8);
    for r in runs {
        let f = r
            .children()
            .find(|n| n.tag_name().name() == "rPr")
            .and_then(|p| p.children().find(|n| n.tag_name().name() == "rFonts"))
            .expect("у каждого run есть rFonts");
        for attr in ["ascii", "hAnsi", "cs", "eastAsia"] {
            assert_eq!(
                f.attributes().find(|a| a.name() == attr).map(|a| a.value()),
                Some("Arial"),
                "rFonts/{attr}"
            );
        }
    }
    let styles = part(&docx, "word/styles.xml");
    assert!(
        styles.contains("Arial") && !styles.contains("Calibri"),
        "в стилях нет Calibri"
    );
}

#[test]
fn title_uses_heading_style_and_footer_has_page_number_field() {
    let docx = build_docx(&sample(), &opts(false)).unwrap();
    assert!(part(&docx, "word/document.xml").contains(r#"w:pStyle w:val="Heading1""#));
    assert!(part(&docx, "word/styles.xml").contains(r#"w:styleId="Heading1""#));
    let footer = part(&docx, "word/footer1.xml");
    assert!(footer.contains("PAGE"), "поле номера страницы: {footer}");
    assert!(part(&docx, "word/document.xml").contains("footerReference"));
}

#[test]
fn special_characters_survive_xml_escaping() {
    let tricky = "Tom & Jerry <b>\"кавычки\"</b> 'ёлка' ©2026 😀 100% a&amp;b";
    let t = transcript(vec![seg(0, 1_000, tricky)]);
    let docx = build_docx(
        &t,
        &ExportOptions {
            title: "Q&A <1>".into(),
            ..opts(false)
        },
    )
    .unwrap();
    let paras = paragraphs_text(&part(&docx, "word/document.xml"));
    assert_eq!(paras[0], "Q&A <1>");
    assert_eq!(paras[2], tricky);
}

#[test]
fn hour_long_transcript_builds_quickly() {
    let segments: Vec<_> = (0..2_000u64)
        .map(|i| {
            seg(
                i * 1_800,
                i * 1_800 + 1_700,
                "Это обычная фраза из длинной записи совещания про планы.",
            )
        })
        .collect();
    let started = std::time::Instant::now();
    let docx = build_docx(&transcript(segments), &opts(true)).unwrap();
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    let paras = paragraphs_text(&part(&docx, "word/document.xml"));
    assert!(paras.len() > 10);
    assert!(docx.len() < 2_000_000);
}

#[test]
fn default_file_name_comes_from_record_name() {
    assert_eq!(default_file_name("Совещание 1.m4a"), "Совещание 1.docx");
    assert_eq!(default_file_name("a/b:c?.mp3"), "b_c_.docx");
    assert_eq!(default_file_name("отчёт: итоги*.wav"), "отчёт_ итоги_.docx");
    assert_eq!(default_file_name(""), "Транскрипт.docx");
    assert_eq!(default_file_name("...mp3"), "Транскрипт.docx");
}

#[test]
fn export_to_file_writes_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("Совещание 1.docx");
    export_to_file(&sample(), &opts(true), &dest).unwrap();
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "временных файлов не осталось"
    );
    assert!(std::fs::metadata(&dest).unwrap().len() > 1_000);

    let missing = dir.path().join("нет").join("x.docx");
    assert!(export_to_file(&sample(), &opts(true), &missing).is_err());
    assert!(!missing.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn independent_parser_textutil_reads_the_document() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.docx");
    export_to_file(&sample(), &opts(true), &dest).unwrap();
    let out = std::process::Command::new("/usr/bin/textutil")
        .args(["-convert", "txt", "-stdout"])
        .arg(&dest)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    for needle in [
        "Совещание 1",
        "[00:00:00] Добрый день, коллеги. Сегодня обсудим план.",
        "[01:02:05] Час два минуты.",
    ] {
        assert!(text.contains(needle), "нет {needle:?} в {text:?}");
    }
}
