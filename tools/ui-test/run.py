#!/usr/bin/env python3
"""Автопроверка очереди (MVP-013/016) в собранном приложении (только macOS; сборка с feature ui-test).

    python3 tools/ui-test/run.py <путь к .app> <каталог с моделями приложения> <файл1> <файл2> <файл3> <длинный файл> [модель]

Приложение работает в отдельном временном каталоге данных (модели подключаются ссылками), данные
пользователя не затрагиваются. Страница выполняет сценарий и пишет шаги в журнал; этот скрипт каждые
50 мс проверяет по таблице процессов, жив ли `tolk-engine`. Прогонов три: основной (заканчивается
закрытием окна во время обработки), затем два запуска «после сбоя» (после закрытия окна и после `kill -9`).
Никаких разрешений системы не нужно.
"""
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).parent


def engine_pids():
    out = subprocess.run(["pgrep", "-x", "tolk-engine"], capture_output=True, text=True).stdout
    return [int(x) for x in out.split()]


class Watcher(threading.Thread):
    def __init__(self):
        super().__init__(daemon=True)
        self.events = []  # (время мс, "up"/"down")
        self.stop = False

    def run(self):
        alive = False
        while not self.stop:
            now = bool(engine_pids())
            if now != alive:
                self.events.append((int(time.time() * 1000), "up" if now else "down"))
                alive = now
            time.sleep(0.05)


def prepare(script_name, subs, tmp, tag):
    text = (HERE / "common.js").read_text() + (HERE / script_name).read_text()
    for k, v in subs.items():
        text = text.replace(k, v)
    path = tmp / f"{tag}-{script_name}"
    path.write_text(text)
    return path


def launch(app, script, log, data, extra_env=None):
    exe = Path(app) / "Contents/MacOS/tolk-app"
    env = dict(os.environ, TOLK_UI_TEST_LOG=str(log), TOLK_UI_TEST_DATA=str(data), **(extra_env or {}))
    if script:
        env["TOLK_UI_TEST_SCRIPT"] = str(script)
    return subprocess.Popen([str(exe)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def read_log(log):
    return [json.loads(l) for l in log.read_text().splitlines()] if log.exists() else []


def wait_step(log, step, timeout):
    t0 = time.time()
    while time.time() < t0 + timeout:
        for e in read_log(log):
            if e["step"] in (step, "error", "timeout"):
                return e
        time.sleep(0.1)
    return None


def show(title, log):
    print(f"--- {title} ---")
    for e in read_log(log):
        print(json.dumps(e, ensure_ascii=False))


def new_data(tmp, name, models, only=None):
    """Каталог данных для прогона; модели подключаются ссылками (`only` — какие именно, по части имени файла)."""
    data = tmp / name
    (data / "models").mkdir(parents=True)
    for f in Path(models).glob("ggml-*.bin"):
        if only is None or any(part in f.name for part in only):
            (data / "models" / f.name).symlink_to(f)
    return data


class DirWatcher(threading.Thread):
    """Следит за числом каталогов заданий на диске: (время мс, число)."""

    def __init__(self, jobs_dir):
        super().__init__(daemon=True)
        self.jobs_dir = jobs_dir
        self.samples = []
        self.stop = False

    def run(self):
        last = None
        while not self.stop:
            n = len([d for d in self.jobs_dir.iterdir() if d.is_dir()]) if self.jobs_dir.exists() else 0
            if n != last:
                self.samples.append((int(time.time() * 1000), n))
                last = n
            time.sleep(0.03)


def list_scenario(app, models, model, f1, tmp):
    data = new_data(tmp, "data-list", models)
    dw = DirWatcher(data / "jobs")
    dw.start()
    log = tmp / "list.log"
    proc = launch(app, prepare("list.js", {"@MODEL@": model, "@F1@": f1}, tmp, "L"), log, data)
    wait_step(log, "L-done", 240)
    proc.kill()
    dw.stop = True
    show("экран очереди и удаление", log)
    steps = {e["step"]: e["t"] for e in read_log(log)}
    print("каталоги заданий на диске (время мс, число):", dw.samples)
    if "L-before-delete" in steps and "L-deleted" in steps:
        at_delete = [n for t, n in dw.samples if t <= steps["L-before-delete"]]
        print(f"каталогов заданий на диске перед удалением: {at_delete[-1] if at_delete else '?'}; после: {dw.samples[-1][1]}")


def models_scenario(app, models, f1, long_, tmp):
    # А. Чистая установка: моделей нет.
    data = new_data(tmp, "data-models-a", models, only=[])
    bad = tmp / "не модель.bin"
    bad.write_bytes(b"x" * 1000)
    small_file = next(Path(models).glob("ggml-small-q5_1.bin"))
    log = tmp / "models-a.log"
    proc = launch(app, prepare("models-a.js", {"@BAD@": str(bad), "@MODELFILE@": str(small_file)}, tmp, "MA"), log, data)
    # Скрипт 3 с ждёт на мастере и только потом импортирует; как только он сообщил о мастере, смотрим на каталог.
    wait_step(log, "M-wizard", 60)
    leftovers = sorted(p.name for p in (data / "models").iterdir())
    print("каталог моделей после запуска на чистой установке (прошло около 3 с на мастере):", leftovers or "пусто")
    wait_step(log, "M-done", 120)
    proc.kill()
    show("первый запуск и импорт модели", log)
    print("после импорта в каталоге моделей:", sorted(p.name for p in (data / "models").iterdir()))

    # Б. Модели small и turbo есть, компонента определения речи нет: настоящая загрузка ~1 МБ.
    data = new_data(tmp, "data-models-b", models, only=["small-q5_1", "turbo"])
    log = tmp / "models-b.log"
    proc = launch(app, prepare("models-b.js", {"@MODEL@": "small-q5_1", "@F1@": f1, "@LONG@": long_}, tmp, "MB"), log, data)
    wait_step(log, "N-done", 400)
    proc.kill()
    show("экран «Модели»", log)
    print("в каталоге моделей:", sorted((p.name, "ссылка" if p.is_symlink() else f"{p.stat().st_size} Б") for p in (data / "models").iterdir()))
    print("исходные файлы моделей целы:", all(p.exists() for p in Path(models).glob("ggml-*.bin")))


def init_db(app, data, tmp):
    """Первый запуск создаёт базу (миграции); дальше её можно заполнять напрямую."""
    proc = launch(app, None, tmp / "seed.log", data)
    for _ in range(100):
        if (data / "tolk.db").exists():
            break
        time.sleep(0.1)
    time.sleep(1.5)
    proc.kill()
    proc.wait()


def seed_transcript_job(data, con, name, segments):
    import uuid

    job_id = str(uuid.uuid4())
    d = data / "jobs" / job_id
    d.mkdir(parents=True)
    doc = {
        "version": 1,
        "engine": "seed",
        "model": "small-q5_1",
        "language": "ru",
        "duration_ms": segments * 4000,
        "segments": [
            {"start_ms": i * 4000, "end_ms": i * 4000 + 3500, "text": f"Сегмент {i + 1}: обсуждаем план работ и сроки проекта."}
            for i in range(segments)
        ],
    }
    text = json.dumps(doc, ensure_ascii=False, indent=2)
    (d / "original.json").write_text(text)
    (d / "transcript.json").write_text(text)
    now = int(time.time() * 1000)
    con.execute(
        "INSERT INTO jobs (id, source_name, audio_path, duration_ms, status, progress, created_at, updated_at, finished_at)"
        " VALUES (?, ?, 'source.mp3', ?, 'COMPLETED', 100, ?, ?, ?)",
        (job_id, name, segments * 4000, now, now, now),
    )
    return job_id


def export_scenario(app, models, model, tmp):
    import sqlite3
    import zipfile

    data = new_data(tmp, "data-export", models)
    init_db(app, data, tmp)
    con = sqlite3.connect(data / "tolk.db")
    with con:
        seed_transcript_job(data, con, "small.mp3", 5)
        seed_transcript_job(data, con, "small2.mp3", 5)
    con.close()
    out = tmp / "экспорт"
    out.mkdir()
    log = tmp / "export.log"
    proc = launch(app, prepare("export.js", {"@MODEL@": model}, tmp, "X"), log, data, {"TOLK_UI_TEST_EXPORT_DIR": str(out)})
    wait_step(log, "X-done", 90)
    proc.kill()
    show("экспорт в DOCX", log)
    print("файлы в каталоге экспорта:", sorted(p.name for p in out.iterdir()))

    def document_xml(path):
        return zipfile.ZipFile(path).read("word/document.xml").decode("utf-8")

    plain, timed = out / "small.docx", out / "small2.docx"
    if plain.exists():
        x = document_xml(plain)
        print("small.docx: правка перед экспортом внутри:", "Правка прямо перед экспортом." in x, "| таймкодов нет:", "[00:00:00]" not in x)
        if sys.platform == "darwin":
            txt = subprocess.run(["textutil", "-convert", "txt", "-stdout", str(plain)], capture_output=True, text=True).stdout
            print("textutil (независимый разбор) читает документ, строк:", len(txt.splitlines()), "| заголовок:", txt.splitlines()[0] if txt else "?")
    if timed.exists():
        x = document_xml(timed)
        print("small2.docx: таймкоды внутри:", "[00:00:00]" in x)


def diag_scenario(app, models, model, voice, tmp):
    import re
    import shutil

    data = new_data(tmp, "data-diag", models)
    out = tmp / "отчёт"
    out.mkdir()
    secret = tmp / "ZZ-Конфиденциальная-запись-Иванов.ogg"
    shutil.copy(voice, secret)
    log = tmp / "diag.log"
    proc = launch(app, prepare("diag.js", {"@MODEL@": model, "@FILE@": str(secret)}, tmp, "D"), log, data, {"TOLK_UI_TEST_EXPORT_DIR": str(out)})
    wait_step(log, "D-done", 180)
    proc.kill()
    show("диагностика и отчёт", log)

    reports = sorted(out.glob("dev-tolk-отчёт-*.json"))
    print("файл отчёта:", [r.name for r in reports])
    if not reports:
        return
    raw = reports[0].read_text()
    rep = json.loads(raw)
    print("формат:", rep["format"], rep["version"], "| ОС:", rep["system"]["os"], rep["system"]["os_version"], "| ЦП:", rep["system"]["cpu"], "| ОЗУ МБ:", rep["system"]["ram_total_mb"])
    job = rep["jobs"][0]
    print("задание в отчёте:", {k: v for k, v in job.items()})
    print("сводка:", rep["summaries"])

    job_dirs = list((data / "jobs").iterdir())
    original = json.loads((job_dirs[0] / "original.json").read_text())
    words = sorted({w.lower() for seg in original["segments"] for w in re.findall(r"[а-яёa-z]{5,}", seg["text"], re.I)})
    print(f"слов расшифровки (5+ букв) для проверки утечек: {len(words)}")
    leaks_report = [w for w in words if w in raw.lower()]
    names_in_report = [n for n in ("Конфиденциальная", "Иванов", "ZZ-", secret.name) if n in raw]
    print("в отчёте найдено слов расшифровки:", leaks_report, "| имён файла:", names_in_report)
    logs = ""
    for f in (data / "logs").iterdir():
        logs += f.read_text(errors="replace").lower()
    leaks_logs = [w for w in words if w in logs]
    print("в журналах найдено слов расшифровки:", leaks_logs, "| размер журналов, байт:", len(logs.encode()))
    print("имя файла в журналах (допустимо):", "конфиденциальная" in logs)

    # Сверка пикового значения с внешним замером тем же движком на том же файле.
    engine = Path(app) / "Contents/MacOS/tolk-engine"
    mdir = data / "models"
    model_file = mdir / f"ggml-{job['model']}.bin" if job.get("model") else None
    if model_file and model_file.exists() and sys.platform == "darwin":
        res = subprocess.run(
            ["/usr/bin/time", "-l", str(engine), "transcribe", "--model", str(model_file), "--vad-model", str(next(mdir.glob("ggml-silero*.bin"))),
             "--input", str(secret), "--output", str(tmp / "ext.json")],
            capture_output=True, text=True,
        )
        m = re.search(r"(\d+)\s+peak memory footprint", res.stderr)
        if m:
            ext_mb = int(m.group(1)) / 1048576
            app_mb = job["peak_rss_mb"]
            print(f"пиковая память: отчёт приложения {app_mb} МБ, внешний замер /usr/bin/time -l {ext_mb:.0f} МБ, расхождение {abs(app_mb - ext_mb) / ext_mb * 100:.1f}%")


def glossary_scenario(app, models, tmp):
    data = new_data(tmp, "data-gl", models, only=[])
    out1 = tmp / "словарь-1"
    out1.mkdir()
    log1 = tmp / "gl1.log"
    proc = launch(app, prepare("gl-1.js", {}, tmp, "G1"), log1, data, {"TOLK_UI_TEST_EXPORT_DIR": str(out1)})
    wait_step(log1, "G-done", 90)
    proc.kill()
    proc.wait()
    show("словарь: добавление, поиск, правка, удаление, экспорт", log1)
    exported = out1 / "словарь.tolk-glossary.json"
    print("файл экспорта:", exported.exists())
    if not exported.exists():
        return
    doc1 = json.loads(exported.read_text())
    print("экспорт: формат", doc1["format"], doc1["version"], "| терминов", len(doc1["terms"]), "|", [(t["term"], t["variants"], t["active"]) for t in doc1["terms"]])

    bad = tmp / "не словарь.tolk-glossary.json"
    bad.write_text("это не JSON, просто текст")
    foreign = tmp / "чужой.tolk-glossary.json"
    foreign.write_text(json.dumps({"format": "other-app", "version": 1, "terms": [{"term": "x"}]}))
    huge = tmp / "огромный.tolk-glossary.json"
    huge.write_bytes(b" " * (1024 * 1024 + 10))
    subs = {"@EXPORTED@": str(exported), "@BAD@": str(bad), "@FOREIGN@": str(foreign), "@HUGE@": str(huge)}
    log2 = tmp / "gl2.log"
    proc = launch(app, prepare("gl-2.js", subs, tmp, "G2"), log2, data)
    wait_step(log2, "G2-done", 60)
    proc.kill()
    proc.wait()
    show("словарь после перезапуска, повторный импорт, плохие файлы", log2)

    data3 = new_data(tmp, "data-gl3", models, only=[])
    out3 = tmp / "словарь-3"
    out3.mkdir()
    log3 = tmp / "gl3.log"
    proc = launch(app, prepare("gl-3.js", subs, tmp, "G3"), log3, data3, {"TOLK_UI_TEST_EXPORT_DIR": str(out3)})
    wait_step(log3, "G3-done", 60)
    proc.kill()
    show("словарь на «другом компьютере»", log3)
    exported3 = out3 / "словарь.tolk-glossary.json"
    if exported3.exists():
        doc3 = json.loads(exported3.read_text())
        key = lambda d: sorted((t["term"], tuple(t["variants"]), t["note"], t["active"]) for t in d["terms"])
        print("экспорт → импорт на другой машине даёт тот же набор терминов:", key(doc1) == key(doc3))


def edit_scenario(app, models, model, tmp):
    import sqlite3

    data = new_data(tmp, "data-edit", models)
    init_db(app, data, tmp)
    con = sqlite3.connect(data / "tolk.db")
    with con:
        small = seed_transcript_job(data, con, "small.mp3", 5)
        long_ = seed_transcript_job(data, con, "long.mp3", 2000)
    con.close()
    subs = {"@MODEL@": model}

    def db_state(job_id):
        c = sqlite3.connect(data / "tolk.db")
        row = c.execute("SELECT has_edits FROM jobs WHERE id = ?", (job_id,)).fetchone()
        c.close()
        return row[0] if row else None

    def segment_texts(job_id, name="transcript.json"):
        return [x["text"] for x in json.loads((data / "jobs" / job_id / name).read_text())["segments"]]

    log = tmp / "edit.log"
    proc = launch(app, prepare("edit.js", subs, tmp, "E"), log, data)
    closing = wait_step(log, "E-closing", 120)
    t0 = time.time()
    exited = None
    if closing and closing["step"] == "E-closing":
        try:
            proc.wait(timeout=15)
            exited = time.time() - t0
        except subprocess.TimeoutExpired:
            proc.kill()
    else:
        proc.kill()
    show("правка транскрипта", log)
    print(f"закрытие окна с несохранённой правкой: приложение завершилось за {exited:.2f} с" if exited is not None else "закрытие окна с несохранённой правкой: приложение НЕ завершилось")
    log_file = data / "logs" / "tolk.log"
    deferred = log_file.exists() and "Закрытие окна отложено" in log_file.read_text()
    print("в журнале приложения есть запись об отложенном закрытии:", deferred)
    texts = segment_texts(small)
    print("на диске после закрытия: сегмент 4 =", texts[3], "| has_edits в базе =", db_state(small),
          "| original.json не изменён:", segment_texts(small, "original.json")[3].startswith("Сегмент 4"))

    log2 = tmp / "edit-verify.log"
    proc = launch(app, prepare("edit-verify.js", subs, tmp, "V"), log2, data)
    wait_step(log2, "V-done", 60)
    proc.kill()
    show("после перезапуска", log2)

    log3 = tmp / "long.log"
    proc = launch(app, prepare("long.js", subs, tmp, "G"), log3, data)
    wait_step(log3, "LONG-done", 120)
    proc.kill()
    show("расшифровка на 2000 сегментов", log3)
    t = segment_texts(long_)
    print("на диске: сегмент 1001 =", t[1000], "| has_edits в базе =", db_state(long_), "| сегментов:", len(t))


def bulk_scenario(app, models, model, tmp, count=200):
    import sqlite3

    data = new_data(tmp, "data-bulk", models)
    log = tmp / "bulk.log"
    # Первый запуск создаёт базу (миграции), затем она заполняется заранее.
    proc = launch(app, None, tmp / "seed.log", data)
    for _ in range(100):
        if (data / "tolk.db").exists():
            break
        time.sleep(0.1)
    time.sleep(1.5)
    proc.kill()
    proc.wait()
    now = int(time.time() * 1000)
    statuses = ["COMPLETED", "COMPLETED", "FAILED", "CANCELLED", "COMPLETED"]
    con = sqlite3.connect(data / "tolk.db")
    with con:
        for i in range(count):
            st = statuses[i % len(statuses)]
            con.execute(
                "INSERT INTO jobs (id, source_name, audio_path, duration_ms, status, progress, error_code, error_message, created_at, updated_at, finished_at)"
                " VALUES (?, ?, 'source.mp3', ?, ?, ?, ?, ?, ?, ?, ?)",
                (f"00000000-0000-4000-8000-{i:012d}", f"Запись {i + 1}.mp3", 600_000 + i * 1000, st, 100 if st == "COMPLETED" else 0,
                 "INTERNAL" if st == "FAILED" else None, "Во время обработки произошла внутренняя ошибка." if st == "FAILED" else None,
                 now - i * 60_000, now - i * 60_000, now - i * 60_000),
            )
    con.close()
    proc = launch(app, prepare("bulk.js", {"@MODEL@": model}, tmp, "B"), log, data)
    wait_step(log, "B200-done", 60)
    proc.kill()
    show(f"список из {count} записей", log)


def import_scenario(app, models, model, tmp):
    fixtures = HERE.parent.parent / "crates/engine/tests/fixtures"
    bad = tmp / "заметка.mp3"
    bad.write_text("это не звук, просто текст")
    subs = {
        "@MODEL@": model,
        "@MP3@": str(fixtures / "mp3_44k.mp3"),
        "@VIDEO@": str(fixtures / "video_aac.mp4"),
        "@BAD@": str(bad),
        "@NOAUDIO@": str(fixtures / "video_noaudio.mp4"),
        "@MISSING@": str(tmp / "нет такого файла.mp3"),
    }
    data = tmp / "data-import"
    (data / "models").mkdir(parents=True)
    for f in Path(models).glob("ggml-*.bin"):
        (data / "models" / f.name).symlink_to(f)
    log = tmp / "import.log"
    proc = launch(app, prepare("import.js", subs, tmp, "0"), log, data)
    wait_step(log, "I-done", 300)
    proc.kill()
    show("импорт", log)


def main():
    app, models, f1, f2, f3, long_ = sys.argv[1:7]
    model = sys.argv[7] if len(sys.argv) > 7 else "large-v3-turbo-q5_0"
    if engine_pids():
        sys.exit("Уже запущен tolk-engine: завершите его перед прогоном")
    subs = {"@MODEL@": model, "@F1@": f1, "@F2@": f2, "@F3@": f3, "@LONG@": long_}
    with tempfile.TemporaryDirectory() as t:
        tmp = Path(t)
        data = tmp / "data"
        (data / "models").mkdir(parents=True)
        for f in Path(models).glob("ggml-*.bin"):
            (data / "models" / f.name).symlink_to(f)
        import_scenario(app, models, model, tmp)
        list_scenario(app, models, model, f1, tmp)
        bulk_scenario(app, models, model, tmp)
        edit_scenario(app, models, model, tmp)
        export_scenario(app, models, model, tmp)
        glossary_scenario(app, models, tmp)
        diag_scenario(app, models, model, f1, tmp)
        models_scenario(app, models, f1, long_, tmp)
        w = Watcher()
        w.start()

        # 1. Основной сценарий, в конце — закрытие окна во время обработки.
        log1 = tmp / "main.log"
        proc = launch(app, prepare("mvp016.js", subs, tmp, "1"), log1, data)
        closing = wait_step(log1, "D-closing", 900)
        t0 = time.time()
        if closing and closing["step"] == "D-closing":
            try:
                proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                proc.kill()
        else:
            proc.kill()
        t_exit = time.time()
        time.sleep(1.0)
        show("основной сценарий", log1)
        print(f"закрытие окна: приложение завершилось за {t_exit - t0:.2f} с; процессов tolk-engine осталось: {engine_pids()}")

        # 2. Запуск после закрытия окна во время обработки: задание «Обработка прервана», повтор работает.
        # Обработку оставляем идти и убиваем приложение (kill -9).
        log2 = tmp / "after-close.log"
        proc = launch(app, prepare("check.js", dict(subs, **{"@CANCEL@": "false"}), tmp, "2"), log2, data)
        ready = wait_step(log2, "check-processing", 300)
        if ready and ready["step"] == "check-processing":
            t_kill = time.time()
            proc.kill()  # SIGKILL
            while time.time() < t_kill + 10 and engine_pids():
                time.sleep(0.05)
            gone = time.time() - t_kill
            print(f"kill -9: движок завершился через {gone * 1000:.0f} мс; осталось: {engine_pids()}")
        else:
            proc.kill()
        show("после закрытия окна", log2)

        # 3. Запуск после kill -9: задание «Обработка прервана», повтор и отмена работают.
        log3 = tmp / "after-kill.log"
        proc = launch(app, prepare("check.js", dict(subs, **{"@CANCEL@": "true"}), tmp, "3"), log3, data)
        wait_step(log3, "check-done", 300)
        proc.kill()
        show("после kill -9", log3)
        time.sleep(0.5)
        w.stop = True
        print("--- процесс tolk-engine (время мс) ---")
        for ts, what in w.events:
            print(ts, what)
        left = engine_pids()
        print("остались процессы tolk-engine:", left)
        for p in left:
            os.kill(p, 9)


if __name__ == "__main__":
    main()
