"""Запуск whisper-cli для конфигураций W1–W9 (BENCHMARK_PLAN §4.1) с замером времени и памяти (macOS).

Замер — через `/usr/bin/time -l`: wall clock, maximum resident set size и peak memory footprint (§5.2).
На Windows замер делается отдельным PowerShell-скриптом (MVP-005); здесь он не реализован.

Входной звук всегда приводится к WAV 16 кГц моно командой `tolk-engine extract`, одинаково для всех
конфигураций (§4.1: whisper-cli не читает M4A). Подготовка звука во время замера не учитывается.
"""

from __future__ import annotations

import csv
import hashlib
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
TOOLS = REPO.parent / "dev-tolk-tools"
WHISPER_CLI = Path(os.environ.get("TOLK_WHISPER_CLI", TOOLS / "whisper.cpp" / "build" / "bin" / "whisper-cli"))
ENGINE = Path(os.environ.get("TOLK_ENGINE", REPO / "target" / "release" / "tolk-engine"))
MODELS_DIR = Path(os.environ.get("TOLK_BENCH_MODELS", TOOLS / "models"))
WORK_DIR = Path(os.environ.get("TOLK_BENCH_WORK", TOOLS / "work"))
VAD_MODEL = "ggml-silero-v6.2.0.bin"

RESULTS_COLUMNS = [
    "date", "machine", "config", "record", "audio_s", "wall_s", "rtf", "max_rss_bytes", "peak_footprint_bytes",
    "wer", "cer", "repeats", "term_recall", "term_false_insertions", "punctuation_score",
    "engine_version", "model_sha256", "threads", "notes",
]


@dataclass(frozen=True)
class Config:
    model: str  # id из models.json
    vad: bool = True
    gpu: bool = True
    glossary_prompt: bool = False
    only_records: tuple[str, ...] = ()
    platforms: tuple[str, ...] = ("darwin", "win32")
    notes: str = ""


# BENCHMARK_PLAN §4.1. G*/P* (sherpa-onnx) запускаются отдельно в MVP-006.
CONFIGS: dict[str, Config] = {
    "W1": Config("large-v3-turbo-q5_0"),
    "W2": Config("large-v3-turbo-q5_0", vad=False, notes="без VAD"),
    "W3": Config("large-v3-turbo-q8_0"),
    "W4": Config("large-v3-turbo-q5_0", gpu=False, notes="только CPU"),
    "W5": Config("large-v3-turbo-q5_0", platforms=("win32",), notes="Vulkan: нужна сборка с Vulkan"),
    "W6": Config("medium-q5_0"),
    "W7": Config("small-q5_1"),
    "W8": Config("large-v3-turbo", only_records=("H2",), notes="f16"),
    "W9": Config("large-v3-turbo-q5_0", glossary_prompt=True, only_records=("A4", "A5"), notes="словарь в подсказке"),
}


def sha256_cached(path: Path) -> str:
    """SHA-256 файла с кэшем в соседнем `.sha256` (модели большие)."""
    side = path.with_name(path.name + ".sha256")
    if side.exists() and side.stat().st_mtime >= path.stat().st_mtime:
        return side.read_text().split()[0]
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    side.write_text(h.hexdigest() + "\n")
    return h.hexdigest()


def prepare_audio(src: Path) -> tuple[Path, float]:
    """WAV 16 кГц моно через tolk-engine; результат кэшируется по SHA-256 исходника. Возвращает путь и длительность в секундах."""
    WORK_DIR.mkdir(parents=True, exist_ok=True)
    digest = hashlib.sha256(src.read_bytes()).hexdigest()[:16]
    wav = WORK_DIR / f"{digest}.wav"
    if not wav.exists():
        proc = subprocess.run([str(ENGINE), "extract", str(src), str(wav)], capture_output=True, text=True)
        if proc.returncode != 0:
            raise SystemExit(f"tolk-engine extract не удался: {proc.stdout.strip().splitlines()[-1:]}")
    seconds = (wav.stat().st_size - 44) / 2 / 16000
    return wav, seconds


def build_command(cfg: Config, wav: Path, out_prefix: Path, threads: int, prompt: str | None) -> list[str]:
    cmd = [str(WHISPER_CLI), "-m", str(MODELS_DIR / f"ggml-{cfg.model}.bin"), "-f", str(wav), "-l", "ru",
           "-mc", "0", "-t", str(threads), "-np", "-nt", "-otxt", "-of", str(out_prefix)]
    if cfg.vad:
        cmd += ["--vad", "-vm", str(MODELS_DIR / VAD_MODEL)]
    if not cfg.gpu:
        cmd.append("-ng")
    if cfg.glossary_prompt:
        if not prompt:
            raise SystemExit("W9 требует --glossary: словарь для подсказки")
        cmd += ["--prompt", prompt, "--carry-initial-prompt"]
    return cmd


@dataclass
class TimeReport:
    wall_s: float
    max_rss_bytes: int
    peak_footprint_bytes: int


def parse_time_l(stderr: str) -> TimeReport:
    """Разбор вывода `/usr/bin/time -l` (macOS)."""
    wall = re.search(r"^\s*([\d.]+)\s+real", stderr, re.M)
    rss = re.search(r"^\s*(\d+)\s+maximum resident set size", stderr, re.M)
    foot = re.search(r"^\s*(\d+)\s+peak memory footprint", stderr, re.M)
    if not (wall and rss and foot):
        raise ValueError("не удалось разобрать вывод time -l")
    return TimeReport(float(wall.group(1)), int(rss.group(1)), int(foot.group(1)))


def glossary_prompt(terms: list[str]) -> str:
    return "Термины: " + ", ".join(terms) + "."


def run(config_id: str, audio: Path, machine: str, record: str, threads: int, terms: list[str] | None,
        dry_run: bool = False) -> dict:
    cfg = CONFIGS[config_id]
    if sys.platform not in cfg.platforms:
        raise SystemExit(f"{config_id} не запускается на {sys.platform}")
    if cfg.only_records and record not in cfg.only_records and machine not in cfg.only_records:
        raise SystemExit(f"{config_id} предусмотрена только для {', '.join(cfg.only_records)}")
    wav, seconds = prepare_audio(audio)
    out_prefix = WORK_DIR / f"{config_id}-{record}-{machine}"
    cmd = build_command(cfg, wav, out_prefix, threads, glossary_prompt(terms) if terms else None)
    if dry_run:
        print(" ".join(["/usr/bin/time", "-l", *cmd]))
        return {}
    model_path = MODELS_DIR / f"ggml-{cfg.model}.bin"
    for needed in (WHISPER_CLI, model_path, *([MODELS_DIR / VAD_MODEL] if cfg.vad else [])):
        if not needed.exists():
            raise SystemExit(f"не найден {needed}")
    proc = subprocess.run(["/usr/bin/time", "-l", *cmd], capture_output=True, text=True)
    if proc.returncode != 0:
        raise SystemExit(f"whisper-cli завершился с кодом {proc.returncode}:\n{proc.stderr[-2000:]}")
    t = parse_time_l(proc.stderr)
    version = subprocess.run([str(WHISPER_CLI), "--version"], capture_output=True, text=True).stdout.strip()
    return {
        "date": date.today().isoformat(), "machine": machine, "config": config_id, "record": record,
        "audio_s": round(seconds, 1), "wall_s": t.wall_s, "rtf": round(t.wall_s / seconds, 4),
        "max_rss_bytes": t.max_rss_bytes, "peak_footprint_bytes": t.peak_footprint_bytes,
        "engine_version": version, "model_sha256": sha256_cached(model_path), "threads": threads,
        "notes": cfg.notes, "_transcript": str(out_prefix) + ".txt",
    }


def append_row(csv_path: Path, row: dict) -> None:
    csv_path.parent.mkdir(parents=True, exist_ok=True)
    new = not csv_path.exists()
    with csv_path.open("a", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=RESULTS_COLUMNS, extrasaction="ignore")
        if new:
            w.writeheader()
        w.writerow(row)
