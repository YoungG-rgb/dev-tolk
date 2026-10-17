"""Сборка длинной записи из клипов Common Voice Spontaneous Speech (CC0) для проверки конвейера.

Клипы одного диктора с проверенной расшифровкой склеиваются в одну запись 16 кГц моно с паузой между
клипами. Это **не замена корпусу MVP-002**: речь — ответы на вопросы, один голос, без переговоров,
шума и перебивания. Подходит для WER на настоящей человеческой эталонной расшифровке и для замера
скорости и памяти на записи в десятки минут.

Эталон: расшифровки клипов подряд. Паразитные протяжки («а-а-а», «и-и-и») убираются из эталона,
потому что распознаватель их не выдаёт; клипы с пометкой `[unclear]` и с тегами качества не берутся.
"""

from __future__ import annotations

import csv
import re
import subprocess
import tempfile
import wave
from pathlib import Path

SAMPLE_RATE = 16000
GAP_S = 0.6
_STRETCH = re.compile(r"(?<!\w)[аэеиымуо](?:-[аэеиымуо])+(?!\w)", re.IGNORECASE)


def pick_clips(rows: list[dict], client_id: str | None, max_minutes: float) -> tuple[str, list[dict]]:
    good = [r for r in rows if r["transcription"] and r["votes"] and not r["quality_tags"] and "[" not in r["transcription"]]
    if client_id is None:
        totals: dict[str, int] = {}
        for r in good:
            totals[r["client_id"]] = totals.get(r["client_id"], 0) + int(r["duration_ms"])
        client_id = max(totals, key=totals.__getitem__)
    chosen, total = [], 0.0
    for r in sorted((r for r in good if r["client_id"] == client_id), key=lambda r: int(r["audio_id"])):
        if total + int(r["duration_ms"]) / 60000 > max_minutes:
            break
        chosen.append(r)
        total += int(r["duration_ms"]) / 60000
    return client_id, chosen


def clean_reference(text: str) -> str:
    return " ".join(_STRETCH.sub(" ", text).split())


def build(src: Path, dest: Path, max_minutes: float = 45, client_id: str | None = None) -> dict:
    with (src / "ss-corpus-ru.tsv").open(encoding="utf-8", newline="") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
    client_id, chosen = pick_clips(rows, client_id, max_minutes)
    if not chosen:
        raise SystemExit("Нет подходящих клипов")
    dest.mkdir(parents=True, exist_ok=True)
    wav_path = dest / "S1.wav"
    gap = b"\x00\x00" * int(SAMPLE_RATE * GAP_S)
    with wave.open(str(wav_path), "wb") as out, tempfile.TemporaryDirectory() as tmp:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(SAMPLE_RATE)
        for r in chosen:
            part = Path(tmp) / "c.wav"
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(src / "audios" / r["audio_file"]), "-ac", "1",
                            "-ar", str(SAMPLE_RATE), "-c:a", "pcm_s16le", str(part)], check=True)
            with wave.open(str(part), "rb") as w:
                out.writeframes(w.readframes(w.getnframes()))
            out.writeframes(gap)
    ref = "\n".join(clean_reference(r["transcription"]) for r in chosen) + "\n"
    (dest / "S1.ref.txt").write_text(ref, encoding="utf-8")
    seconds = sum(int(r["duration_ms"]) for r in chosen) / 1000 + GAP_S * len(chosen)
    (dest / "S1.clips.txt").write_text("\n".join(r["audio_file"] for r in chosen) + "\n", encoding="utf-8")
    return {"wav": wav_path, "ref": dest / "S1.ref.txt", "clips": len(chosen), "seconds": seconds,
            "words": len(ref.split()), "speaker": client_id[:6]}
