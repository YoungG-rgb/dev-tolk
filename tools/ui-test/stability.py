#!/usr/bin/env python3
"""Испытания устойчивости MVP-027 на собранном приложении (macOS, сборка с feature ui-test).

    python3 tools/ui-test/stability.py <.app> <каталог моделей> <длинная запись .wav> [--only queue,kill,import,save,full] [--count 6]

Каждое испытание работает в своём временном каталоге данных; данные пользователя не затрагиваются.
Что проверяется: очередь из длинных записей без вмешательства (память, запрет сна), `kill -9` приложения
в обработке, в импорте и в сохранении правки, заполненный диск. Итог каждого испытания: целостность базы
(`PRAGMA integrity_check`), нет «осиротевших» файлов и записей без файлов, прерванное можно повторить.
"""
import argparse
import json
import os
import random
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import run  # noqa: E402

HERE = Path(__file__).parent


def db_report(data, title):
    """Целостность базы и согласованность с файлами. Возвращает список замечаний."""
    problems = []
    db = data / "tolk.db"
    if not db.exists():
        print(f"[{title}] базы нет")
        return ["нет базы"]
    con = sqlite3.connect(db)
    integrity = con.execute("PRAGMA integrity_check").fetchone()[0]
    if integrity != "ok":
        problems.append(f"integrity_check: {integrity}")
    rows = con.execute("SELECT id, status, audio_path, error_code FROM jobs").fetchall()
    ids = {r[0] for r in rows}
    jobs_dir = data / "jobs"
    dirs = {d.name for d in jobs_dir.iterdir() if d.is_dir()} if jobs_dir.exists() else set()
    orphans = dirs - ids
    missing = [r[0][:8] for r in rows if not (jobs_dir / r[0] / (r[2] or "-")).exists()]
    counts = {}
    for r in rows:
        counts[r[1]] = counts.get(r[1], 0) + 1
    tmp = data / "tmp"
    leftovers = sorted(p.name for p in tmp.iterdir()) if tmp.exists() else []
    con.close()
    if orphans:
        problems.append(f"каталогов заданий без записи: {len(orphans)}")
    if missing:
        problems.append(f"записей без аудио: {missing}")
    print(f"[{title}] база: integrity={integrity}; задания: {counts}; без записи каталогов: {len(orphans)}; "
          f"записей без аудио: {len(missing)}; файлов в tmp: {len(leftovers)}")
    return problems


def kill_app(proc):
    proc.kill()  # SIGKILL
    proc.wait()
    deadline = time.time() + 5
    while time.time() < deadline and run.engine_pids():
        time.sleep(0.05)
    left = run.engine_pids()
    for p in left:
        os.kill(p, 9)
    return left


def rss_mb(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return int(out) / 1024 if out else None


def sleep_assertions():
    out = subprocess.run(["pmset", "-g", "assertions"], capture_output=True, text=True).stdout
    return [l.strip() for l in out.splitlines() if "tolk" in l.lower()]


def test_queue(app, models, wav, count, tmp):
    print(f"\n=== Очередь из {count} длинных записей без вмешательства")
    data = run.new_data(tmp, "data-queue", models)
    files = []
    for i in range(count):
        f = tmp / f"запись-{i + 1}.wav"
        shutil.copy(wav, f)
        files.append(f)
    subs = {"@MODEL@": "large-v3-turbo-q5_0", "@FILES@": ", ".join(json.dumps(str(f), ensure_ascii=False) for f in files), "@COUNT@": str(count)}
    log = tmp / "queue.log"
    proc = run.launch(app, run.prepare("stab-queue.js", subs, tmp, "SQ"), log, data)
    t0 = time.time()
    rss, assertions_mid = [], None
    watcher = run.Watcher()
    watcher.start()
    done = None
    while time.time() - t0 < 3 * 3600:
        for e in run.read_log(log):
            if e["step"] in ("Q-done", "error", "timeout"):
                done = e
        if done:
            break
        r = rss_mb(proc.pid)
        if r:
            rss.append(r)
        if assertions_mid is None and time.time() - t0 > 90:
            assertions_mid = sleep_assertions()
        time.sleep(5)
    watcher.stop = True
    after = sleep_assertions()
    kill_app(proc)
    run.show("ход очереди", log)
    problems = db_report(data, "очередь")
    con = sqlite3.connect(data / "tolk.db")
    metrics = con.execute("SELECT audio_ms, processing_ms, peak_rss_bytes, backend, outcome FROM job_metrics ORDER BY id").fetchall()
    con.close()
    for m in metrics:
        print(f"  запись {m[0] / 60000:.1f} мин: обработка {m[1] / 1000:.0f} с (RTF {m[1] / m[0]:.3f}), пик движка {m[2] / 1048576:.0f} МБ, {m[3]}, {m[4]}")
    if rss:
        print(f"память процесса приложения (tolk-app), МБ: в начале {rss[0]:.0f}, максимум {max(rss):.0f}, в конце {rss[-1]:.0f}")
    print("запрет сна во время обработки (pmset -g assertions):", assertions_mid)
    print("запрет сна после окончания очереди:", after or "снят")
    if not done or done["step"] != "Q-done":
        problems.append("очередь не завершилась")
    return problems


def test_kill_processing(app, models, wav, tmp):
    print("\n=== Принудительное завершение (kill -9) в обработке; повторные завершения подряд")
    data = run.new_data(tmp, "data-kill", models)
    fixtures = HERE.parent.parent / "crates/engine/tests/fixtures"
    files = [wav, fixtures / "mp3_44k.mp3", fixtures / "opus.ogg"]
    subs = {"@MODEL@": "large-v3-turbo-q5_0", "@FILES@": ", ".join(json.dumps(str(f), ensure_ascii=False) for f in files), "@NOPROGRESS@": "false"}
    problems = []
    log = tmp / "kill1.log"
    proc = run.launch(app, run.prepare("stab-start.js", subs, tmp, "K1"), log, data)
    ready = run.wait_step(log, "K-ready", 180)
    if not ready or ready["step"] != "K-ready":
        proc.kill()
        return ["сценарий не дошёл до обработки"]
    left = kill_app(proc)
    print("1) kill -9 в обработке (одна идёт, две в очереди): процессов движка осталось:", left)
    problems += db_report(data, "после 1-го kill")
    # Второй запуск сразу убиваем, не дав доработать (убийство во время восстановления и старта очереди).
    for n in (2, 3):
        p2 = run.launch(app, None, tmp / f"k{n}.log", data)
        time.sleep(0.4 + 0.3 * n)
        left = kill_app(p2)
        print(f"{n}) повторный запуск и kill -9 через {0.4 + 0.3 * n:.1f} с: процессов движка осталось: {left}")
        problems += db_report(data, f"после {n}-го kill")
    log2 = tmp / "kill-after.log"
    p3 = run.launch(app, run.prepare("stab-after.js", subs, tmp, "KA"), log2, data)
    run.wait_step(log2, "A-done", 25 * 60)
    kill_app(p3)
    run.show("после перезапуска: список, повтор, завершение", log2)
    problems += db_report(data, "итог")
    return problems


def test_import_kill(app, models, wav, tmp):
    print("\n=== kill -9 во время пакетного импорта (разные моменты)")
    fixtures = HERE.parent.parent / "crates/engine/tests/fixtures"
    files = [wav, fixtures / "video_aac.mp4", fixtures / "mp3_44k.mp3", wav, fixtures / "opus.ogg", wav]
    subs = {"@MODEL@": "large-v3-turbo-q5_0", "@FILES@": ", ".join(json.dumps(str(f), ensure_ascii=False) for f in files)}
    problems = []
    for delay in (0.05, 0.12, 0.25, 0.5, 0.9, 1.5):
        data = run.new_data(tmp, f"data-import-{delay}", models, only=["silero"])
        # В этом испытании модели распознавания нет: задания не берутся в работу, проверяется только импорт.
        log = tmp / f"imp-{delay}.log"
        proc = run.launch(app, run.prepare("stab-import.js", subs, tmp, f"SI{delay}"), log, data)
        run.wait_step(log, "I-sent", 30)
        time.sleep(delay)
        kill_app(proc)
        p2 = run.launch(app, None, tmp / "imp-restart.log", data)
        time.sleep(2.5)
        kill_app(p2)
        problems += db_report(data, f"импорт, kill через {delay} с")
    return problems


def test_save_kill(app, models, tmp, trials=8):
    print(f"\n=== kill -9 во время непрерывного сохранения правок ({trials} попыток)")
    data = run.new_data(tmp, "data-save", models, only=[])
    run.init_db(app, data, tmp)
    con = sqlite3.connect(data / "tolk.db")
    with con:
        job_id = run.seed_transcript_job(data, con, "long.mp3", 2000)
    con.close()
    problems = []
    original = (data / "jobs" / job_id / "original.json").read_bytes()
    for trial in range(trials):
        log = tmp / f"save-{trial}.log"
        proc = run.launch(app, run.prepare("stab-save.js", {}, tmp, f"SS{trial}"), log, data)
        run.wait_step(log, "S-start", 30)
        time.sleep(random.uniform(0.3, 3.0))
        last = max([e.get("n", 0) for e in run.read_log(log) if e["step"] == "S-iter"] or [0])
        kill_app(proc)
        path = data / "jobs" / job_id / "transcript.json"
        try:
            doc = json.loads(path.read_text())
            n = int(doc["segments"][0]["text"].split("№")[1]) if "№" in doc["segments"][0]["text"] else 0
            m = int(doc["segments"][1000]["text"].split("№")[1]) if "№" in doc["segments"][1000]["text"] else 0
            ok = len(doc["segments"]) == 2000 and n == m and n >= last
            print(f"  попытка {trial + 1}: файл целый, на диске правка №{n}, сообщено сохранённых до kill ≥ {last}, согласован: {ok}")
            if not ok:
                problems.append(f"попытка {trial + 1}: файл целый, но n={n}, m={m}, last={last}")
        except Exception as e:  # noqa: BLE001
            problems.append(f"попытка {trial + 1}: transcript.json повреждён: {e}")
            print(f"  попытка {trial + 1}: ПОВРЕЖДЁН {e}")
        if (data / "jobs" / job_id / "original.json").read_bytes() != original:
            problems.append("original.json изменился")
        left = [p.name for p in (data / "tmp").iterdir()] if (data / "tmp").exists() else []
        if left:
            print("   остатки в tmp (уберёт очистка при запуске):", left[:3])
    # has_edits после серии
    con = sqlite3.connect(data / "tolk.db")
    he = con.execute("SELECT has_edits FROM jobs WHERE id = ?", (job_id,)).fetchone()[0]
    con.close()
    print("has_edits в базе после серии:", he)
    problems += [p for p in db_report(data, "после серии") if not p.startswith("записей без аудио")]  # у заполненного вручную задания аудио нет
    return problems


def test_full_disk(app, models, f1, f2, tmp):
    print("\n=== Заполненный диск (отдельный том 60 МБ)")
    image = tmp / "small.dmg"
    mount = tmp / "smallvol"
    mount.mkdir()
    subprocess.run(["hdiutil", "create", "-size", "60m", "-fs", "HFS+", "-volname", "tolkfull", str(image)], check=True, capture_output=True)
    subprocess.run(["hdiutil", "attach", "-nobrowse", "-noautoopen", "-mountpoint", str(mount), str(image)], check=True, capture_output=True)
    problems = []
    try:
        data = mount / "data"
        (data / "models").mkdir(parents=True)
        for f in Path(models).glob("ggml-*.bin"):
            if "small" in f.name or "silero" in f.name:
                (data / "models" / f.name).symlink_to(f)
        subs = {"@MODEL@": "small-q5_1", "@F1@": str(f1), "@F2@": str(f2)}
        log1 = tmp / "full1.log"
        proc = run.launch(app, run.prepare("stab-full1.js", subs, tmp, "F1"), log1, data)
        run.wait_step(log1, "F1-ready", 120)
        kill_app(proc)
        # Заполняем том до отказа.
        filler = mount / "filler.bin"
        subprocess.run(["dd", "if=/dev/zero", f"of={filler}", "bs=1m"], capture_output=True)
        # Остаток меньше мегабайта добиваем мелкими файлами, чтобы свободного места не осталось совсем.
        crumbs = mount / "crumbs"
        crumbs.mkdir()
        for i in range(2000):
            try:
                (crumbs / f"c{i}").write_bytes(b"x" * 4096)
            except OSError:
                break
        free = shutil.disk_usage(mount).free
        print(f"том заполнен, свободно {free} байт")
        log2 = tmp / "full2.log"
        proc = run.launch(app, run.prepare("stab-full2.js", subs, tmp, "F2"), log2, data)
        failed = run.wait_step(log2, "F2-save-failed", 90)
        if failed and failed["step"] == "F2-save-failed":
            filler.unlink()  # освобождаем место: повтор сохранения должен пройти
            shutil.rmtree(crumbs, ignore_errors=True)
            print("место освобождено")
        run.wait_step(log2, "F2-done", 120)
        kill_app(proc)
        run.show("заполненный диск", log2)
        steps = {e["step"]: e for e in run.read_log(log2)}
        if steps.get("F2-saved-after-free", {}).get("state") != "saved":
            problems.append("правка не сохранилась после освобождения места")
        texts = [s["text"] for s in json.loads((next((data / "jobs").iterdir()) / "transcript.json").read_text())["segments"]]
        print("на диске после освобождения места: первый фрагмент =", texts[0])
        if texts[0] != "Правка при заполненном диске.":
            problems.append("правка не оказалась на диске")
        problems += db_report(data, "заполненный диск")
    finally:
        subprocess.run(["hdiutil", "detach", str(mount), "-force"], capture_output=True)
    return problems


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("app")
    ap.add_argument("models")
    ap.add_argument("wav")
    ap.add_argument("--only", default="queue,kill,import,save,full")
    ap.add_argument("--count", type=int, default=6)
    a = ap.parse_args()
    if run.engine_pids():
        sys.exit("Уже запущен tolk-engine")
    only = set(a.only.split(","))
    fixtures = HERE.parent.parent / "crates/engine/tests/fixtures"
    voice_dir = Path.home() / "corpus/voice"
    results = {}
    with tempfile.TemporaryDirectory() as t:
        tmp = Path(t)
        if "kill" in only:
            results["kill"] = test_kill_processing(a.app, a.models, a.wav, tmp)
        if "import" in only:
            results["import"] = test_import_kill(a.app, a.models, a.wav, tmp)
        if "save" in only:
            results["save"] = test_save_kill(a.app, a.models, tmp)
        if "full" in only:
            results["full"] = test_full_disk(a.app, a.models, voice_dir / "V1.ogg", voice_dir / "V2.ogg", tmp)
        if "queue" in only:
            results["queue"] = test_queue(a.app, a.models, a.wav, a.count, tmp)
    print("\n=== ИТОГ")
    for name, problems in results.items():
        print(f"{name}: {'без замечаний' if not problems else problems}")


if __name__ == "__main__":
    main()
