//! Каталог моделей, сведения о системе и предварительная рекомендация модели (TD-13, TD-14, MVP-022).
//!
//! Каталог встроен в приложение (`catalog/models.json`): пользователь не вводит произвольные URL.
//! Адреса закреплены на коммит репозитория, поэтому файл по ссылке не может незаметно смениться,
//! а размер и SHA-256 всегда соответствуют версии.

use std::sync::OnceLock;

use serde::Deserialize;

const CATALOG_JSON: &str = include_str!("../catalog/models.json");

/// Модель, которую приложение рекомендует по умолчанию (TD-02): ждёт подтверждения бенчмарком.
pub const DEFAULT_MODEL_ID: &str = "large-v3-turbo-q5_0";
/// Модель определения речи, устанавливается вместе с основной (TD-13).
pub const VAD_MODEL_ID: &str = "silero-vad-v6.2.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    /// Модель распознавания речи.
    Asr,
    /// Модель определения речи (VAD).
    Vad,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub kind: ModelKind,
    pub title: String,
    pub engine: String,
    /// Источник для показа пользователю, например «Hugging Face: ggerganov/whisper.cpp».
    pub source: String,
    pub repo: String,
    /// Закреплённая ревизия репозитория (версия модели).
    pub revision: String,
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub license: String,
    /// Ориентировочная память, МБ. `None` — не оценивалось. Это [Оценка], не измерение.
    #[serde(default)]
    pub ram_estimate_mb: Option<u32>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Deserialize)]
struct Catalog {
    models: Vec<ModelEntry>,
}

/// Встроенный каталог. Разбор проверяется тестами, поэтому ошибка здесь — дефект сборки.
pub fn catalog() -> &'static [ModelEntry] {
    static CATALOG: OnceLock<Vec<ModelEntry>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str::<Catalog>(CATALOG_JSON)
            .expect("встроенный каталог моделей повреждён")
            .models
    })
}

pub fn find(id: &str) -> Option<&'static ModelEntry> {
    catalog().iter().find(|m| m.id == id)
}

// ---- система -------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    pub os: &'static str,
    pub arch: &'static str,
    /// Всего оперативной памяти, байт.
    pub total_ram_bytes: u64,
    /// Доступно сейчас, байт. На macOS это приблизительная оценка (сжатая память, unified memory).
    pub available_ram_bytes: u64,
    /// Поддержка AVX2; `None`, если это не x86-64 (Apple Silicon: вопрос не стоит).
    pub avx2: Option<bool>,
}

impl SystemInfo {
    pub fn detect() -> Self {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            total_ram_bytes: sys.total_memory(),
            available_ram_bytes: sys.available_memory(),
            avx2: detect_avx2(),
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn detect_avx2() -> Option<bool> {
    Some(std::arch::is_x86_feature_detected!("avx2"))
}

#[cfg(not(target_arch = "x86_64"))]
fn detect_avx2() -> Option<bool> {
    None
}

// ---- рекомендация --------------------------------------------------------------------------------------

const GIB: u64 = 1024 * 1024 * 1024;

/// Класс по объёму памяти. Граница проходит посередине между классами: машина «8 ГБ» показывает
/// меньше 8 ГиБ, «16 ГБ» — чуть меньше 16 и т. д. **Пороги предварительные**: окончательные
/// фиксируются по измерениям (TD-14, MVP-007, MVP-038).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RamClass {
    Gb8,
    Gb16,
    Gb24,
}

impl RamClass {
    pub fn from_total_bytes(total: u64) -> Self {
        if total <= 12 * GIB {
            RamClass::Gb8
        } else if total <= 20 * GIB {
            RamClass::Gb16
        } else {
            RamClass::Gb24
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommendation {
    pub model_id: &'static str,
    pub ram_class: RamClass,
    /// Рекомендация предварительная: пороги не подтверждены измерениями. Всегда `true` до MVP-007.
    pub preliminary: bool,
    /// Класс 8 ГБ: показывать предупреждение о нехватке памяти при открытых программах (TD-14).
    pub low_memory_warning: bool,
}

/// Предварительная рекомендация по TD-14: turbo q5_0 для всех классов; для 8 ГБ — с предупреждением.
/// Запасные варианты для 8 ГБ (medium q5_0, small q5_1, GigaAM) выбираются по данным и пока не рекомендуются.
pub fn recommend(info: &SystemInfo) -> Recommendation {
    let ram_class = RamClass::from_total_bytes(info.total_ram_bytes);
    Recommendation {
        model_id: DEFAULT_MODEL_ID,
        ram_class,
        preliminary: true,
        low_memory_warning: ram_class == RamClass::Gb8,
    }
}
