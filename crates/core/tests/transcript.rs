use tolk_core::transcript::{Segment, Transcript, FORMAT_VERSION};
use tolk_core::StoreError;

fn sample() -> Transcript {
    Transcript {
        version: FORMAT_VERSION,
        engine: "whisper.cpp 1.9.5".into(),
        model: "large-v3-turbo-q5_0".into(),
        language: "ru".into(),
        duration_ms: 5_000,
        segments: vec![Segment {
            start_ms: 0,
            end_ms: 5_000,
            text: "Привет, мир".into(),
        }],
    }
}

#[test]
fn json_roundtrip_keeps_cyrillic() {
    let t = sample();
    let bytes = t.to_json().unwrap();
    assert!(
        String::from_utf8(bytes.clone())
            .unwrap()
            .contains("Привет, мир"),
        "кириллица не экранируется"
    );
    assert_eq!(Transcript::from_json(&bytes).unwrap(), t);
}

#[test]
fn damaged_or_foreign_files_are_rejected() {
    for bad in [
        &b""[..],
        b"not json",
        b"{}",
        br#"{"version":1}"#,
        b"[1,2,3]",
    ] {
        assert!(
            matches!(
                Transcript::from_json(bad),
                Err(StoreError::BadTranscript(_))
            ),
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
    let mut future = sample();
    future.version = 99;
    assert!(matches!(
        Transcript::from_json(&future.to_json().unwrap()),
        Err(StoreError::BadTranscript(_))
    ));

    let mut reversed = sample();
    reversed.segments[0].end_ms = 0;
    reversed.segments[0].start_ms = 10;
    assert!(
        matches!(Transcript::from_json(&reversed.to_json().unwrap()), Err(StoreError::BadTranscript(m)) if m.contains("сегмент 0"))
    );
}
