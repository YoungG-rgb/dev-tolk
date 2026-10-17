"""Помощники для подготовки корпуса (MVP-002): нарезка эталонного фрагмента с черновиком и заполнение manifest.csv.

Ручная работа остаётся человеку: исправить каждое слово в `*.ref.txt`, прослушать второй раз, указать дату согласия.
Скрипты ничего не придумывают за человека: ни согласия, ни проверки эталона.
"""

from __future__ import annotations

import csv
import hashlib
import json
import subprocess
import tempfile
import wave
from pathlib import Path

from . import corpus, whisper_run

FRAGMENT_SECONDS = 300
DRAFT_MODEL_ID = "large-v3-q5_0"  # модель-черновик, не участвующая в сравнении (BENCHMARK_PLAN §2.2)
MANIFEST_COLUMNS = ["id", "file", "kind", "duration_s", "container_codec", "source_device", "speakers",
                    "environment", "consent_date", "sha256", "notes"]


def parse_time(text: str) -> float:
    """`1200`, `20:00` или `1:05:30` -> секунды."""
    parts = text.strip().split(":")
    if not 1 <= len(parts) <= 3 or not all(p.replace(".", "", 1).isdigit() for p in parts):
        raise ValueError(f"не удалось разобрать время {text!r}: ожидается секунды, мм:сс или чч:мм:сс")
    seconds = 0.0
    for p in parts:
        seconds = seconds * 60 + float(p)
    return seconds


def slice_wav(src: Path, dest: Path, start_s: float, duration_s: float) -> float:
    """Вырезает кусок WAV (PCM) и атомарно записывает его. Возвращает фактическую длительность в секундах."""
    with wave.open(str(src), "rb") as r:
        params = r.getparams()
        rate = params.framerate
        total = params.nframes
        first = int(start_s * rate)
        count = int(duration_s * rate)
        if first >= total:
            raise ValueError(f"начало {start_s:.0f} с за пределами записи ({total / rate:.0f} с)")
        if first + count > total:
            raise ValueError(f"от {start_s:.0f} с до конца записи меньше {duration_s:.0f} с ({(total - first) / rate:.0f} с)")
        r.setpos(first)
        frames = r.readframes(count)
    dest.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=dest.parent, suffix=".part", delete=False) as tmp:
        tmp_path = Path(tmp.name)
    try:
        with wave.open(str(tmp_path), "wb") as w:
            w.setparams(params)
            w.writeframes(frames)
        tmp_path.replace(dest)
    finally:
        tmp_path.unlink(missing_ok=True)
    return count / rate


def draft_text(original_json: Path) -> str:
    """Текст черновика: по одному сегменту в строке (удобно править)."""
    data = json.loads(original_json.read_text(encoding="utf-8"))
    return "\n".join(s["text"].strip() for s in data["segments"] if s["text"].strip()) + "\n"


def prepare_fragment(root: Path, rid: str, start_s: float, reviewer: str = "", force: bool = False,
                     threads: int = 4) -> dict:
    """Нарезает 5-минутный фрагмент `rid` и делает черновик моделью large-v3-q5_0."""
    if rid not in corpus.FRAGMENT_SOURCES:
        raise SystemExit(f"эталонные фрагменты делаются только для {', '.join(corpus.FRAGMENT_SOURCES)}")
    exts = corpus.RECORDINGS[rid][0]
    recording = corpus._find_recording(root, rid, exts)
    if recording is None:
        raise SystemExit(f"нет записи recordings/{rid}{'|'.join(exts)}")
    base = root / "fragments" / f"{rid}f"
    wav, draft, ref, meta = (base.with_suffix(s) for s in (".wav", ".draft.txt", ".ref.txt", ".json"))
    if wav.exists() and not force:
        raise SystemExit(f"{wav.name} уже есть; --force перезапишет только wav, черновик и метаданные эталона (ref.txt не трогается)")
    draft_model = whisper_run.MODELS_DIR / f"ggml-{DRAFT_MODEL_ID}.bin"
    vad_model = whisper_run.MODELS_DIR / whisper_run.VAD_MODEL
    for needed in (draft_model, vad_model, whisper_run.ENGINE):
        if not needed.exists():
            raise SystemExit(f"не найден {needed}")

    full_wav, total_s = whisper_run.prepare_audio(recording)
    actual = slice_wav(full_wav, wav, start_s, FRAGMENT_SECONDS)

    out = base.with_suffix(".draft.json")
    proc = subprocess.run([str(whisper_run.ENGINE), "transcribe", "--model", str(draft_model), "--vad-model", str(vad_model),
                           "--input", str(wav), "--output", str(out), "--threads", str(threads)],
                          capture_output=True, text=True)
    if proc.returncode != 0:
        tail = proc.stdout.strip().splitlines()[-1:] or [proc.stderr.strip()[-200:]]
        raise SystemExit(f"черновик не получен (код {proc.returncode}): {tail}")
    draft.write_text(draft_text(out), encoding="utf-8")
    out.unlink()

    created_ref = False
    if not ref.exists():
        ref.write_text(draft.read_text(encoding="utf-8"), encoding="utf-8")  # заготовка для ручной правки
        created_ref = True
    if not meta.exists() or force:
        previous = json.loads(meta.read_text(encoding="utf-8")) if meta.exists() else {}
        meta.write_text(json.dumps({
            "source": rid, "start_ms": int(start_s * 1000), "draft_model": DRAFT_MODEL_ID,
            "reviewer": reviewer or previous.get("reviewer", ""), "review2_done": False,
        }, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return {"wav": str(wav), "seconds": actual, "recording_seconds": total_s, "draft": str(draft),
            "ref": str(ref), "ref_created_from_draft": created_ref, "meta": str(meta)}


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def build_manifest(root: Path, existing: list[dict], probe=corpus.engine_probe, consent_date: str = "") -> list[dict]:
    """Строки manifest.csv по файлам корпуса. Уже заполненные человеком поля сохраняются; SHA-256, длительность
    и кодек обновляются по файлам. Дата согласия берётся только из `consent_date` или из прежней строки."""
    old = {row["id"]: row for row in existing}
    rows: list[dict] = []
    items: list[tuple[str, Path, str]] = []
    for rid, (exts, *_rest) in corpus.RECORDINGS.items():
        path = corpus._find_recording(root, rid, exts)
        if path:
            items.append((rid, path, "recording"))
    for rid in corpus.FRAGMENT_SOURCES:
        path = root / "fragments" / f"{rid}f.wav"
        if path.is_file():
            items.append((f"{rid}f", path, "fragment"))
    for rid, path, kind in items:
        info = probe(path)
        row = {c: "" for c in MANIFEST_COLUMNS}
        row.update({k: v for k, v in old.get(rid, {}).items() if k in MANIFEST_COLUMNS and v})
        row.update({
            "id": rid, "file": str(path.relative_to(root)), "kind": kind, "sha256": _sha256(path),
            "duration_s": f"{(info.get('duration_ms') or 0) / 1000:.0f}" if info.get("type") == "probe" else "",
            "container_codec": (f"{info.get('container', '')}/{info.get('codec', '')}" if info.get("type") == "probe" else ""),
        })
        if kind == "recording" and consent_date and not row["consent_date"]:
            row["consent_date"] = consent_date
        rows.append(row)
    return rows


def write_manifest(root: Path, consent_date: str = "") -> list[dict]:
    path = root / "manifest.csv"
    existing: list[dict] = []
    if path.is_file():
        with path.open(encoding="utf-8", newline="") as f:
            existing = list(csv.DictReader(f))
    rows = build_manifest(root, existing, consent_date=consent_date)
    with path.open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=MANIFEST_COLUMNS)
        w.writeheader()
        w.writerows(rows)
    return rows
