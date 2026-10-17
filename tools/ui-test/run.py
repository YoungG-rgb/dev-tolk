#!/usr/bin/env python3
"""Прогон сценариев MVP-013 в собранном приложении (только macOS; нужна сборка с feature ui-test).

    python3 tools/ui-test/run.py <путь к .app> <короткий файл> <длинный файл> [модель]

Страница выполняет сценарий и пишет шаги в журнал; параллельно этот скрипт каждые 50 мс проверяет
по таблице процессов, жив ли `tolk-engine`, и сверяет время. Никаких разрешений системы не нужно.
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


def prepare(script_name, short, long_, model, tmp):
    text = (HERE / "common.js").read_text() + (HERE / script_name).read_text()
    text = text.replace("@SHORT@", short).replace("@LONG@", long_).replace("@MODEL@", model)
    path = tmp / script_name
    path.write_text(text)
    return path


def launch(app, script, log):
    exe = Path(app) / "Contents/MacOS/tolk-app"
    env = dict(os.environ, TOLK_UI_TEST_SCRIPT=str(script), TOLK_UI_TEST_LOG=str(log))
    return subprocess.Popen([str(exe)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def read_log(log):
    return [json.loads(l) for l in log.read_text().splitlines()] if log.exists() else []


def wait_step(log, step, timeout):
    t0 = time.time()
    while time.time() - t0 < timeout:
        for e in read_log(log):
            if e["step"] in (step, "error", "timeout"):
                return e
        time.sleep(0.1)
    return None


def scenario_main(app, short, long_, model, tmp):
    log = tmp / "main.log"
    w = Watcher()
    w.start()
    proc = launch(app, prepare("mvp013.js", short, long_, model, tmp), log)
    closing = wait_step(log, "D-closing", 600)
    t_close = None
    if closing and closing["step"] == "D-closing":
        t_close = int(time.time() * 1000)
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            proc.kill()
    else:
        proc.kill()
    t_exit = int(time.time() * 1000)
    time.sleep(1.0)
    w.stop = True
    leftover = engine_pids()
    print("--- шаги страницы ---")
    for e in read_log(log):
        print(json.dumps(e, ensure_ascii=False))
    print("--- процесс tolk-engine (время мс) ---")
    for t, what in w.events:
        print(t, what)
    cancel = next((e for e in read_log(log) if e["step"] == "B-cancel-clicked"), None)
    downs = [t for t, what in w.events if what == "down"]
    if cancel:
        after = [t for t in downs if t >= cancel["t"]]
        if after:
            print(f"ОТМЕНА: от клика до исчезновения процесса {after[0] - cancel['t']} мс")
    if t_close and downs:
        print(f"ЗАКРЫТИЕ: приложение завершилось через {t_exit - t_close} мс после закрытия окна; "
              f"последний останов движка через {downs[-1] - t_close} мс")
    print("приложение завершено:", proc.poll() is not None, "| остались процессы tolk-engine:", leftover)


def scenario_kill(app, short, long_, model, tmp):
    log = tmp / "kill.log"
    w = Watcher()
    w.start()
    proc = launch(app, prepare("hang.js", short, long_, model, tmp), log)
    ready = wait_step(log, "hang-ready", 300)
    if not ready or ready["step"] != "hang-ready":
        proc.kill()
        print("KILL -9: сценарий не дошёл до обработки", read_log(log))
        return
    t_kill = int(time.time() * 1000)
    proc.kill()  # SIGKILL
    deadline = time.time() + 10
    while time.time() < deadline and engine_pids():
        time.sleep(0.05)
    t_gone = int(time.time() * 1000)
    w.stop = True
    left = engine_pids()
    print(f"KILL -9: движок завершился через {t_gone - t_kill} мс" if not left else f"KILL -9: движок остался: {left}")
    for p in left:
        os.kill(p, 9)


if __name__ == "__main__":
    app, short, long_ = sys.argv[1:4]
    model = sys.argv[4] if len(sys.argv) > 4 else "large-v3-turbo-q5_0"
    if engine_pids():
        sys.exit("Уже запущен tolk-engine: завершите его перед прогоном")
    with tempfile.TemporaryDirectory() as t:
        tmp = Path(t)
        scenario_main(app, short, long_, model, tmp)
        print()
        scenario_kill(app, short, long_, model, tmp)
