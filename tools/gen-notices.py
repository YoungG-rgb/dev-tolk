#!/usr/bin/env python3
"""Собирает THIRD_PARTY_NOTICES.md: лицензии всего, что попадает в поставку (MVP-029).

    python3 tools/gen-notices.py            # пишет THIRD_PARTY_NOTICES.md в корень репозитория
    python3 tools/gen-notices.py --check    # только проверяет, что файл актуален (для CI)

Источники:
- Rust: `cargo metadata` по зависимостям `tolk-app` и `tolk-engine` (обычные зависимости, без сборочных и
  тестовых) для macOS и Windows; тексты лицензий берутся из файлов LICENSE*, COPYING*, NOTICE* в каталоге пакета;
- npm: `@tauri-apps/api` и `svelte` с зависимостями из `app/node_modules` (в сборку попадает их код);
- вручную: whisper.cpp и ggml, libopus, модели (источники и права из `crates/core/catalog/models.json`).

Одинаковые тексты лицензий выводятся один раз со списком пакетов. Крейты без файла лицензии перечислены отдельно
с указанием SPDX-лицензии и авторов.
"""
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "THIRD_PARTY_NOTICES.md"
LICENSE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE")


def license_files(directory: Path):
    if not directory.is_dir():
        return []
    return sorted(
        f for f in directory.iterdir()
        if f.is_file() and f.name.upper().startswith(LICENSE_PREFIXES) and f.stat().st_size < 200_000
    )


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace").strip().replace("\r\n", "\n")


def rust_packages():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", "aarch64-apple-darwin", "--filter-platform", "x86_64-pc-windows-msvc"],
        capture_output=True, text=True, check=True, cwd=ROOT,
    ).stdout
    meta = json.loads(out)
    pk = {p["id"]: p for p in meta["packages"]}
    ws = set(meta["workspace_members"])
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    stack = [i for i in ws if pk[i]["name"] in ("tolk-app", "tolk-engine")]
    seen = set()
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(k["kind"] is None for k in d["dep_kinds"]):
                stack.append(d["pkg"])
    return sorted((pk[i] for i in seen if i not in ws), key=lambda p: (p["name"], p["version"]))


def npm_packages():
    nm = ROOT / "app" / "node_modules"
    wanted, queue = {}, ["@tauri-apps/api", "svelte"]
    while queue:
        name = queue.pop()
        if name in wanted:
            continue
        pj = nm / name / "package.json"
        if not pj.exists():
            continue
        data = json.loads(pj.read_text(encoding="utf-8"))
        wanted[name] = (data, nm / name)
        queue.extend(data.get("dependencies", {}).keys())
    return wanted


def main():
    check = "--check" in sys.argv
    crates = rust_packages()
    groups = {}  # sha -> (text, [descr])
    no_file = []

    def add(text, descr):
        key = hashlib.sha256(text.encode()).hexdigest()
        groups.setdefault(key, (text, []))[1].append(descr)

    for p in crates:
        d = Path(p["manifest_path"]).parent
        files = license_files(d)
        descr = f"{p['name']} {p['version']} ({p['license'] or 'лицензия не указана'})"
        if not files:
            no_file.append((p, descr))
            continue
        for f in files:
            add(read_text(f), descr)

    npm = npm_packages()
    for name, (data, d) in sorted(npm.items()):
        files = license_files(d)
        descr = f"{name} {data.get('version', '?')} ({data.get('license', '?')}) [npm]"
        if not files:
            no_file.append(({"name": name, "version": data.get("version"), "license": data.get("license"), "authors": [], "repository": ""}, descr))
            continue
        for f in files:
            add(read_text(f), descr)

    # whisper.cpp и ggml (вендорятся в whisper-rs-sys) и libopus (в opusic-sys): тексты лежат в подкаталогах.
    vendored = []
    for p in crates:
        d = Path(p["manifest_path"]).parent
        if p["name"] == "whisper-rs-sys":
            vendored.append(("whisper.cpp и ggml (вшиты в whisper-rs-sys " + p["version"] + ")", d / "whisper.cpp"))
        if p["name"] == "opusic-sys":
            vendored.append(("libopus (вшит в opusic-sys " + p["version"] + ")", d / "opus"))

    lines = [
        "# Уведомления о сторонних компонентах (THIRD_PARTY_NOTICES)",
        "",
        "dev-tolk включает перечисленные ниже компоненты. Файл создаётся скриптом `tools/gen-notices.py`; руками не править.",
        "Лицензия самого dev-tolk не выбрана.",
        "",
        "## 1. Модели и вшитые библиотеки",
        "",
        "- **Модели Whisper** (OpenAI) в формате ggml: лицензия MIT, источник `huggingface.co/ggerganov/whisper.cpp` (закреплённые ревизии указаны в каталоге моделей приложения). Копирайт: Copyright (c) 2022 OpenAI. Модели **не входят в установщик**: пользователь скачивает их сам по кнопке или импортирует из файла.",
        "- **Silero VAD** (модель определения речи в формате ggml): лицензия MIT, Copyright (c) 2020-present Silero Team; источник `huggingface.co/ggml-org/whisper-vad` (ревизия закреплена в каталоге). Файл (885 098 байт) **входит в приложение**.",
        "- **whisper.cpp и ggml** (распознавание, вшиты в `tolk-engine` через `whisper-rs-sys`): лицензия MIT, текст ниже.",
        "- **libopus** (декодирование Opus, вшит через `opusic-sys`): лицензия BSD-3-Clause, текст ниже.",
        "- **symphonia** (декодирование аудио и видеоконтейнеров): лицензия **MPL-2.0**. Исходный код symphonia не изменялся; его исходники доступны по адресу https://github.com/pdeljanov/Symphonia (версия указана в списке ниже). Файлы под MPL-2.0 распространяются в составе исполняемого файла `tolk-engine` без изменений.",
        "",
    ]
    for title, d in vendored:
        files = license_files(d)
        lines += [f"### {title}", ""]
        if not files:
            lines += ["Файл лицензии в пакете не найден.", ""]
        for f in files:
            lines += ["```text", read_text(f), "```", ""]
    lines += [
        "## 2. Пакеты Rust и npm, попадающие в поставку",
        "",
        f"Всего пакетов Rust: {len(crates)}; npm: {len(npm)}. Ниже каждый уникальный текст лицензии приведён один раз, под ним перечислены пакеты, у которых он есть.",
        "",
    ]
    for n, (text, descrs) in enumerate(sorted(groups.values(), key=lambda g: (-len(g[1]), g[0][:40])), 1):
        uniq = sorted(set(descrs))
        lines += [f"### Текст {n} ({len(uniq)} пакетов)", "", "Пакеты: " + "; ".join(uniq), "", "```text", text, "```", ""]
    if no_file:
        lines += ["## 3. Пакеты без файла лицензии в архиве", "", "Для них указана лицензия из метаданных пакета.", ""]
        for p, descr in no_file:
            authors = ", ".join(p.get("authors") or []) or "авторы не указаны"
            lines.append(f"- {descr}; {authors}; {p.get('repository') or 'репозиторий не указан'}")
        lines.append("")
    text = "\n".join(lines).rstrip() + "\n"
    if check:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            sys.exit("THIRD_PARTY_NOTICES.md устарел: запустите python3 tools/gen-notices.py")
        print("THIRD_PARTY_NOTICES.md актуален")
        return
    OUT.write_text(text, encoding="utf-8")
    print(f"{OUT.name}: {len(text) / 1024:.0f} КБ, пакетов Rust {len(crates)}, npm {len(npm)}, уникальных текстов {len(groups)}, без файла лицензии {len(no_file)}")


if __name__ == "__main__":
    main()
