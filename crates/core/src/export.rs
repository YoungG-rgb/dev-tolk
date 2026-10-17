//! Экспорт транскрипта в DOCX без Microsoft Word (TD-12).
//!
//! Документ: заголовок (стиль «Заголовок 1»), строка метаданных, абзацы, сгруппированные по паузам,
//! опциональные таймкоды `[чч:мм:сс]`, номера страниц в нижнем колонтитуле. Шрифт Arial задан явно для
//! `ascii`, `hAnsi`, `cs` и `eastAsia`: он есть на Windows и macOS, а Calibri на macOS без Office — нет.

use std::io::{Cursor, Write};
use std::path::Path;

use docx_rs::{
    AlignmentType, Docx, Footer, LineSpacing, PageNum, Paragraph, Run, RunFonts, Style, StyleType,
};

use crate::error::{Result, StoreError};
use crate::transcript::Transcript;

pub const FONT: &str = "Arial";

/// Пауза между сегментами, после которой начинается новый абзац.
pub const PARAGRAPH_PAUSE_MS: u64 = 2_000;
/// Абзац длиннее этого числа символов закрывается на ближайшей границе сегментов, даже без паузы.
pub const PARAGRAPH_MAX_CHARS: usize = 1_200;

const BODY_SIZE: usize = 22; // 11 пт в половинках пункта
const TITLE_SIZE: usize = 32;
const META_SIZE: usize = 18;
/// Интервалы в двадцатых долях пункта (twips): после заголовка, после метаданных, после абзаца.
const SPACE_AFTER_TITLE: u32 = 80;
const SPACE_AFTER_META: u32 = 280;
const SPACE_AFTER_BODY: u32 = 160;
/// Межстрочный интервал 1,15 (в 240-х долях строки).
const LINE_SPACING: i32 = 276;
const GREY: &str = "666666";

#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Заголовок документа — имя записи.
    pub title: String,
    /// Дата для строки метаданных (уже отформатированная вызывающим кодом).
    pub date: String,
    /// Добавлять `[чч:мм:сс]` в начале каждого абзаца.
    pub timestamps: bool,
}

/// Абзац документа: время начала первого сегмента и текст.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub start_ms: u64,
    pub text: String,
}

/// Группирует сегменты в абзацы по паузам. Пустые сегменты пропускаются, пробелы нормализуются.
pub fn group_paragraphs(t: &Transcript) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut prev_end: Option<u64> = None;
    for seg in &t.segments {
        let text = seg.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        let new_paragraph = match (blocks.last(), prev_end) {
            (None, _) | (_, None) => true,
            (Some(last), Some(end)) => {
                seg.start_ms.saturating_sub(end) >= PARAGRAPH_PAUSE_MS
                    || last.text.chars().count() >= PARAGRAPH_MAX_CHARS
            }
        };
        if new_paragraph {
            blocks.push(Block {
                start_ms: seg.start_ms,
                text,
            });
        } else if let Some(last) = blocks.last_mut() {
            last.text.push(' ');
            last.text.push_str(&text);
        }
        prev_end = Some(seg.end_ms);
    }
    blocks
}

/// `чч:мм:сс` для времени в миллисекундах.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

fn fonts() -> RunFonts {
    RunFonts::new()
        .ascii(FONT)
        .hi_ansi(FONT)
        .cs(FONT)
        .east_asia(FONT)
}

fn run(text: &str) -> Run {
    Run::new().add_text(text).fonts(fonts()).size(BODY_SIZE)
}

/// Собирает DOCX в памяти.
pub fn build_docx(t: &Transcript, opts: &ExportOptions) -> Result<Vec<u8>> {
    let heading = Style::new("Heading1", StyleType::Paragraph)
        .name("heading 1")
        .fonts(fonts())
        .size(TITLE_SIZE)
        .bold();

    let meta = format!(
        "Дата: {} · Длительность: {} · Модель: {}",
        opts.date,
        clock(t.duration_ms),
        t.model
    );

    let mut doc = Docx::new()
        .default_fonts(fonts())
        .default_size(BODY_SIZE)
        .add_style(heading)
        .footer(
            Footer::new().add_paragraph(
                Paragraph::new()
                    .align(AlignmentType::Center)
                    .add_run(
                        Run::new()
                            .add_text("Страница ")
                            .fonts(fonts())
                            .size(META_SIZE),
                    )
                    .add_page_num(PageNum::new()),
            ),
        )
        .add_paragraph(
            Paragraph::new()
                .style("Heading1")
                .line_spacing(LineSpacing::new().after(SPACE_AFTER_TITLE))
                .add_run(
                    Run::new()
                        .add_text(&opts.title)
                        .fonts(fonts())
                        .size(TITLE_SIZE)
                        .bold(),
                ),
        )
        .add_paragraph(
            Paragraph::new()
                .line_spacing(LineSpacing::new().after(SPACE_AFTER_META))
                .add_run(
                    Run::new()
                        .add_text(meta)
                        .fonts(fonts())
                        .size(META_SIZE)
                        .color(GREY),
                ),
        );

    for block in group_paragraphs(t) {
        let mut p = Paragraph::new().line_spacing(
            LineSpacing::new()
                .after(SPACE_AFTER_BODY)
                .line(LINE_SPACING),
        );
        if opts.timestamps {
            p = p.add_run(
                Run::new()
                    .add_text(format!("[{}] ", clock(block.start_ms)))
                    .fonts(fonts())
                    .size(BODY_SIZE)
                    .color(GREY),
            );
        }
        doc = doc.add_paragraph(p.add_run(run(&block.text)));
    }

    let mut buf = Cursor::new(Vec::new());
    doc.build()
        .pack(&mut buf)
        .map_err(|e| StoreError::Export(e.to_string()))?;
    Ok(buf.into_inner())
}

/// Имя файла по умолчанию из имени записи: без расширения исходного файла и без недопустимых символов.
pub fn default_file_name(title: &str) -> String {
    let stem = Path::new(title)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(title);
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim();
    let base = if cleaned.is_empty() {
        "Транскрипт"
    } else {
        cleaned
    };
    let base: String = base.chars().take(120).collect();
    format!("{base}.docx")
}

/// Атомарно записывает файл для пользователя: временный файл рядом, затем переименование.
/// Права файла обычные (0644 на Unix), а не 0600, как у временных файлов.
pub fn write_file_atomic(bytes: &[u8], dest: &Path) -> Result<()> {
    let dir = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    crate::disk::ensure_space(dir, bytes.len() as u64)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".tolk-export-")
        .suffix(".part")
        .tempfile_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o644))?;
    }
    tmp.persist(dest).map_err(|e| StoreError::Io(e.error))?;
    Ok(())
}

/// Формирует документ и атомарно записывает его в `dest`.
pub fn export_to_file(t: &Transcript, opts: &ExportOptions, dest: &Path) -> Result<()> {
    write_file_atomic(&build_docx(t, opts)?, dest)
}
