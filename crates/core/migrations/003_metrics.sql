-- Схема v3: метрики производительности заданий (MVP-038). Только числа и коды: ни имён файлов, ни текстов.
-- Строки не привязаны внешним ключом к заданиям и переживают удаление записи: в них нет содержимого.

CREATE TABLE job_metrics (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id         TEXT NOT NULL UNIQUE,
    audio_ms       INTEGER NOT NULL CHECK (audio_ms >= 0),
    processing_ms  INTEGER NOT NULL CHECK (processing_ms >= 0),
    peak_rss_bytes INTEGER CHECK (peak_rss_bytes IS NULL OR peak_rss_bytes >= 0),
    backend        TEXT CHECK (backend IS NULL OR backend IN ('metal', 'vulkan', 'cpu')),
    model_id       TEXT,
    outcome        TEXT NOT NULL CHECK (outcome IN ('SUCCESS', 'FAILED', 'CANCELLED')),
    error_code     TEXT,
    user_rating    INTEGER CHECK (user_rating IS NULL OR user_rating BETWEEN 1 AND 5),
    recorded_at    INTEGER NOT NULL
) STRICT;
