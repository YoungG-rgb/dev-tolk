-- Схема v1: задания и настройки. Аудио и тексты транскриптов в базе не хранятся.
-- Время — Unix-время в миллисекундах (UTC).

CREATE TABLE jobs (
    id            TEXT PRIMARY KEY NOT NULL,
    source_name   TEXT NOT NULL,              -- имя исходного файла для показа пользователю
    audio_path    TEXT,                       -- копия аудио, относительно jobs/<id>/
    duration_ms   INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    status        TEXT NOT NULL DEFAULT 'QUEUED'
                  CHECK (status IN ('QUEUED', 'PROCESSING', 'COMPLETED', 'FAILED', 'CANCELLED')),
    progress      INTEGER NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 100),
    model_id      TEXT,
    error_code    TEXT,
    error_message TEXT,
    attempts      INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    has_edits     INTEGER NOT NULL DEFAULT 0 CHECK (has_edits IN (0, 1)),
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    started_at    INTEGER,
    finished_at   INTEGER
) STRICT;

CREATE INDEX jobs_status ON jobs (status);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;
