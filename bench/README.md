# bench: инструменты замеров

Только стандартная библиотека Python 3.9+. Это инструменты разработчика: на машинах сотрудников Python не нужен (BENCHMARK_PLAN §2.2).
Записи, эталоны и модели лежат вне репозитория, здесь только код. Что именно измеряется и как — в `docs/BENCHMARK_PLAN.md`.

Команды запускаются из каталога `bench/`:

```bash
python3 -m unittest discover -s tests          # самопроверка инструментов
python3 fetch_models.py --list                 # манифест моделей (размеры, SHA-256, источники)
python3 fetch_models.py large-v3-turbo-q5_0 silero-vad-v6.2.0          # показать план загрузки
python3 fetch_models.py large-v3-turbo-q5_0 silero-vad-v6.2.0 --yes    # загрузить (только после согласия)

python3 -m tolkbench hashes ~/corpus           # SHA-256 файлов корпуса (MVP-002)
python3 -m tolkbench whisper W1 ~/corpus/a1.m4a --machine H2 --record A1 --threads 4
python3 -m tolkbench whisper W9 ~/corpus/a5.m4a --machine H2 --record A5 --glossary terms.json
python3 -m tolkbench score out.txt --ref ref.txt --glossary terms.json    # WER, CER, повторы, термины
```

## Как устроено

- `normalize.py` — единая нормализация эталона и результата; список междометий `FILLERS` фиксируется в отчёте.
- `metrics.py` — WER с разбивкой замен, удалений и вставок; CER по алгоритму Хиррё (быстро на 5-минутных фрагментах).
- `repeats.py` — зацикливания: подряд повторяющиеся n-граммы (n ≥ 4, не менее 3 повторов).
- `terms.py` — точность терминов словаря и ложные вставки (W9, MVP-037).
- `whisper_run.py` — конфигурации W1–W9, запуск `whisper-cli` под `/usr/bin/time -l`, строка в `docs/bench/results.csv` (колонки §5.3). Звук всегда приводится к WAV 16 кГц моно командой `tolk-engine extract` (нужна `cargo build --release -p tolk-engine`).
- Пути по умолчанию: whisper-cli — `../dev-tolk-tools/whisper.cpp/build/bin/whisper-cli`, модели — `../dev-tolk-tools/models`; переопределяются переменными `TOLK_WHISPER_CLI`, `TOLK_BENCH_MODELS`, `TOLK_ENGINE`, `TOLK_BENCH_WORK`.

## Ограничения

- Замер времени и памяти реализован только для macOS (`time -l`). На Windows нужен PowerShell-скрипт (MVP-005).
- Запуск `whisper-cli` на реальной модели ещё не проверялся: моделей и записей пока нет. Проверены разбор вывода `time -l`, сборка команд и метрики (`tests/`).
- G1/G2/P1 (sherpa-onnx) здесь не запускаются: это MVP-006.
