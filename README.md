# dev-tolk

Офлайн-приложение для расшифровки русских записей (совещания, звонки, голосовые) на компьютере сотрудника: Tauri 2 + Svelte 5, ядро на Rust, распознавание в отдельном процессе `tolk-engine` (whisper.cpp). Записи и тексты не покидают компьютер; сеть нужна только для загрузки модели по кнопке.

Состояние: MVP в работе, версия 0.1.0, проверено на macOS (Apple Silicon). Windows отложен и ни разу не собирался. Что сделано и что проверено — в [docs/TASKS.md](docs/TASKS.md).

## Документы

| | |
|---|---|
| [docs/ROADMAP.md](docs/ROADMAP.md) | цели, этапы, ворота решений |
| [docs/TASKS.md](docs/TASKS.md) | задачи MVP-001…038, статусы и результаты проверок |
| [docs/TECHNICAL_DECISIONS.md](docs/TECHNICAL_DECISIONS.md) | принятые технические решения (TD-xx) |
| [docs/ENGINE_PROTOCOL.md](docs/ENGINE_PROTOCOL.md) | протокол `tolk-engine` (JSON Lines) |
| [docs/BENCHMARK_PLAN.md](docs/BENCHMARK_PLAN.md), [docs/CORPUS_REQUIREMENTS.md](docs/CORPUS_REQUIREMENTS.md) | бенчмарк и требования к тестовому корпусу |
| [docs/USER_GUIDE.md](docs/USER_GUIDE.md) | инструкция для сотрудников |

## Структура

- `crates/engine` — `tolk-engine`: `probe`, `extract`, `transcribe` (Symphonia, libopus, rubato, whisper-rs).
- `crates/core` — `tolk-core`: SQLite, очередь и исполнитель, импорт, правка, экспорт DOCX, модели, словарь, метрики.
- `app` — приложение Tauri (`app/src` — Svelte, `app/src-tauri` — оболочка).
- `bench` — инструменты бенчмарка на Python (только стандартная библиотека).
- `tools` — `prepare-sidecar.sh` (сайдкар и модель определения речи для сборки), `ui-test` (автопроверка собранного приложения).

## Сборка (macOS)

Нужны: Rust (stable), Node.js 22, CMake, Xcode Command Line Tools.

```bash
tools/prepare-sidecar.sh               # собирает tolk-engine и готовит ресурсы приложения
cd app && npm ci
npx tauri build --bundles app          # .app; для .dmg: --bundles dmg
```

Сайдкар и модель определения речи (885 КБ) в репозиторий не входят: их готовит `tools/prepare-sidecar.sh` (модель берётся из `../dev-tolk-tools/models`, если есть, иначе скачивается с закреплённого адреса с проверкой SHA-256). Без этого `cargo build/clippy` приложения не проходят.

## Проверки

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p tolk-engine -p tolk-core
(cd bench && python3 -m unittest discover -s tests)
(cd app && npm run check)
cargo deny check licenses bans sources
```

Тесты с настоящими моделями пропускаются, если моделей нет (`$TOLK_TEST_MODELS` или `../dev-tolk-tools/models`: `small-q5_1` и `silero-vad`; скачать: `python3 bench/fetch_models.py small-q5_1 silero-vad-v6.2.0 --yes`). Модели вне репозитория, тестовый корпус тоже.

Автопроверка собранного приложения в настоящем окне (macOS): сборка с `--features ui-test`, затем `tools/ui-test/run.py` (описание — в начале файла). Эта сборка в выпуск не входит.

## Лицензия

Лицензия проекта не выбрана (`UNLICENSED`). Лицензии зависимостей и моделей проверяются `cargo deny` и фиксируются в MVP-029.
