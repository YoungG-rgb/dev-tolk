use std::collections::HashSet;

use tolk_core::models::{
    catalog, find, recommend, ModelKind, RamClass, SystemInfo, DEFAULT_MODEL_ID, VAD_MODEL_ID,
};

const GIB: u64 = 1024 * 1024 * 1024;

fn system(total: u64) -> SystemInfo {
    SystemInfo {
        os: "windows",
        arch: "x86_64",
        total_ram_bytes: total,
        available_ram_bytes: total / 2,
        avx2: Some(true),
    }
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[test]
fn catalog_entries_are_complete_and_pinned() {
    let models = catalog();
    assert!(models.len() >= 7);
    let mut ids = HashSet::new();
    for m in models {
        assert!(ids.insert(m.id.as_str()), "повтор id {}", m.id);
        assert!(is_hex(&m.sha256, 64), "{}: SHA-256 {}", m.id, m.sha256);
        assert!(
            is_hex(&m.revision, 40),
            "{}: ревизия не закреплена на коммит",
            m.id
        );
        assert!(
            m.size > 0 && m.file.ends_with(".bin") && !m.title.is_empty() && !m.license.is_empty(),
            "{}",
            m.id
        );
        assert!(
            m.source.starts_with("Hugging Face: ") && m.source.ends_with(&m.repo),
            "{}: источник {}",
            m.id,
            m.source
        );
        assert_eq!(
            m.url,
            format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                m.repo, m.revision, m.file
            ),
            "{}",
            m.id
        );
        assert!(
            !m.url.contains("/main/"),
            "{}: адрес должен ссылаться на коммит, а не на main",
            m.id
        );
        assert_eq!(m.engine, "whisper.cpp");
    }
}

#[test]
fn default_and_vad_models_exist_and_have_expected_kinds() {
    assert_eq!(find(DEFAULT_MODEL_ID).unwrap().kind, ModelKind::Asr);
    assert_eq!(find(VAD_MODEL_ID).unwrap().kind, ModelKind::Vad);
    assert_eq!(
        catalog()
            .iter()
            .filter(|m| m.kind == ModelKind::Vad)
            .count(),
        1
    );
    assert!(find("нет такой").is_none());
}

#[test]
fn catalog_facts_from_the_research_are_preserved() {
    let turbo = find("large-v3-turbo-q5_0").unwrap();
    assert_eq!(turbo.size, 574_041_195);
    assert_eq!(
        turbo.sha256,
        "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"
    );
    assert_eq!(find(VAD_MODEL_ID).unwrap().size, 885_098);
    // Память — только там, где есть основания для оценки.
    assert_eq!(turbo.ram_estimate_mb, Some(1200));
    assert_eq!(find("small-q5_1").unwrap().ram_estimate_mb, None);
}

#[test]
fn ram_classes_split_in_the_middle_between_nominal_sizes() {
    // Машина «8 ГБ» показывает чуть меньше 8 ГиБ и т. д.
    assert_eq!(
        RamClass::from_total_bytes(7 * GIB + GIB * 7 / 10),
        RamClass::Gb8
    );
    assert_eq!(RamClass::from_total_bytes(12 * GIB), RamClass::Gb8);
    assert_eq!(RamClass::from_total_bytes(12 * GIB + 1), RamClass::Gb16);
    assert_eq!(
        RamClass::from_total_bytes(15 * GIB + GIB * 7 / 10),
        RamClass::Gb16
    );
    assert_eq!(RamClass::from_total_bytes(20 * GIB), RamClass::Gb16);
    assert_eq!(RamClass::from_total_bytes(20 * GIB + 1), RamClass::Gb24);
    assert_eq!(RamClass::from_total_bytes(24 * GIB), RamClass::Gb24);
    assert_eq!(RamClass::from_total_bytes(0), RamClass::Gb8);
}

/// Таблица TD-14 (предварительная): turbo q5_0 для 8, 16 и 24 ГБ; для 8 ГБ — с предупреждением.
#[test]
fn recommendation_matches_td14_table() {
    for (total, class, warning) in [
        (8 * GIB, RamClass::Gb8, true),
        (16 * GIB, RamClass::Gb16, false),
        (24 * GIB, RamClass::Gb24, false),
    ] {
        let r = recommend(&system(total));
        assert_eq!(r.model_id, "large-v3-turbo-q5_0", "{total}");
        assert_eq!(
            (r.ram_class, r.low_memory_warning),
            (class, warning),
            "{total}"
        );
        assert!(r.preliminary, "до измерений рекомендация предварительная");
        assert!(find(r.model_id).is_some());
    }
}

#[test]
fn system_detection_returns_sane_values() {
    let s = SystemInfo::detect();
    assert!(!s.os.is_empty() && !s.arch.is_empty());
    assert!(
        s.total_ram_bytes >= GIB,
        "объём памяти не определён: {}",
        s.total_ram_bytes
    );
    assert!(s.available_ram_bytes <= s.total_ram_bytes);
    if cfg!(target_arch = "aarch64") {
        assert_eq!(s.avx2, None);
    } else {
        assert!(s.avx2.is_some());
    }
}

#[test]
fn required_downloads_lists_model_and_vad_until_installed() {
    use tolk_core::download::required_downloads;
    let dir = tempfile::tempdir().unwrap();
    let ids = |v: Vec<&tolk_core::models::ModelEntry>| {
        v.into_iter().map(|m| m.id.clone()).collect::<Vec<_>>()
    };
    assert_eq!(
        ids(required_downloads(DEFAULT_MODEL_ID, dir.path()).unwrap()),
        vec![DEFAULT_MODEL_ID, VAD_MODEL_ID]
    );

    // Установленной считается модель с файлом правильного размера.
    let vad = find(VAD_MODEL_ID).unwrap();
    std::fs::write(dir.path().join(&vad.file), vec![0u8; vad.size as usize]).unwrap();
    assert_eq!(
        ids(required_downloads(DEFAULT_MODEL_ID, dir.path()).unwrap()),
        vec![DEFAULT_MODEL_ID]
    );
    assert!(required_downloads("нет такой", dir.path()).is_none());
}
