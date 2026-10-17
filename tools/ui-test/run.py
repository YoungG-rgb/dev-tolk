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


def launch(app, script, log, data):
    exe = Path(app) / "Contents/MacOS/tolk-app"
    env = dict(os.environ, TOLK_UI_TEST_SCRIPT=str(script), TOLK_UI_TEST_LOG=str(log), TOLK_UI_TEST_DATA=str(data))
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
