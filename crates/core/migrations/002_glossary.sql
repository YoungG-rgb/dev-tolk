-- Схема v2: словарь терминов (TD-18, MVP-036).
-- term_key — нормализованное написание (регистр, ё -> е, пробелы): по нему определяются совпадения.
-- variants — JSON-массив строк: как термин может быть распознан или записан иначе.

CREATE TABLE glossary_terms (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    term       TEXT NOT NULL,
    term_key   TEXT NOT NULL UNIQUE,
    variants   TEXT NOT NULL DEFAULT '[]',
    note       TEXT NOT NULL DEFAULT '',
    active     INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
