#!/usr/bin/env python3
"""Загрузка моделей для замеров с проверкой размера и SHA-256.

Ничего не скачивается без явного указания: без --yes скрипт только показывает, что и откуда будет загружено.

  python3 bench/fetch_models.py --list
  python3 bench/fetch_models.py large-v3-turbo-q5_0 silero-vad-v6.2.0          # показать план
  python3 bench/fetch_models.py large-v3-turbo-q5_0 silero-vad-v6.2.0 --yes    # загрузить

Каталог по умолчанию — вне репозитория: $TOLK_BENCH_MODELS или ../dev-tolk-tools/models.
Незавершённая загрузка лежит в <файл>.part и докачивается (Range), итоговый файл появляется только после проверки.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_DIR = Path(os.environ.get("TOLK_BENCH_MODELS", HERE.parent.parent / "dev-tolk-tools" / "models"))


def load_manifest() -> dict[str, dict]:
    return {m["id"]: m for m in json.loads((HERE / "models.json").read_text(encoding="utf-8"))["models"]}


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def fetch(model: dict, dest_dir: Path) -> Path:
    dest_dir.mkdir(parents=True, exist_ok=True)
    final = dest_dir / model["file"]
    if final.exists() and final.stat().st_size == model["size"] and sha256_file(final) == model["sha256"]:
        print(f"{model['id']}: уже есть и проверен")
        return final
    part = final.with_name(final.name + ".part")
    have = part.stat().st_size if part.exists() else 0
    req = urllib.request.Request(model["url"], headers={"Range": f"bytes={have}-"} if have else {})
    with urllib.request.urlopen(req, timeout=60) as resp:
        if have and resp.status != 206:  # сервер не поддержал докачку
            have = 0
        mode = "ab" if have else "wb"
        with part.open(mode) as out:
            done, shown = have, -1
            while block := resp.read(1 << 20):
                out.write(block)
                done += len(block)
                decile = done * 10 // model["size"]
                if decile != shown:  # не чаще раза на 10%
                    shown = decile
                    print(f"{model['id']}: {done / 1e6:.0f} / {model['size'] / 1e6:.0f} МБ", flush=True)
    if part.stat().st_size != model["size"]:
        raise SystemExit(f"{model['id']}: размер {part.stat().st_size} вместо {model['size']}; файл оставлен как .part")
    actual = sha256_file(part)
    if actual != model["sha256"]:
        part.unlink()
        raise SystemExit(f"{model['id']}: SHA-256 не совпал ({actual}); файл удалён")
    part.replace(final)
    print(f"{model['id']}: загружен и проверен")
    return final


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("ids", nargs="*", help="идентификаторы моделей из models.json")
    ap.add_argument("--list", action="store_true", help="показать манифест")
    ap.add_argument("--yes", action="store_true", help="подтвердить загрузку")
    ap.add_argument("--dir", type=Path, default=DEFAULT_DIR)
    args = ap.parse_args()
    manifest = load_manifest()
    if args.list or not args.ids:
        for m in manifest.values():
            print(f"{m['id']:22} {m['size'] / 1e6:8.1f} МБ  {m['license']:4}  {m['url']}")
        return
    unknown = [i for i in args.ids if i not in manifest]
    if unknown:
        sys.exit(f"неизвестные модели: {', '.join(unknown)}")
    total = sum(manifest[i]["size"] for i in args.ids)
    print(f"Каталог: {args.dir}\nК загрузке: {total / 1e6:.0f} МБ")
    for i in args.ids:
        print(f"  {i}: {manifest[i]['size'] / 1e6:.0f} МБ из {manifest[i]['url']}")
    if not args.yes:
        print("\nЗагрузка не выполнена: добавьте --yes, чтобы подтвердить.")
        return
    for i in args.ids:
        fetch(manifest[i], args.dir)


if __name__ == "__main__":
    main()
