"""Проверка корпуса на соответствие docs/CORPUS_REQUIREMENTS.md (MVP-002).

Ничего не меняет и не копирует: только читает корпус и печатает отчёт. Аудио остаётся вне репозитория.
"""

from __future__ import annotations

import csv
import hashlib
import json
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

from . import whisper_run
from .normalize import normalize
from .terms import _count

# id -> (расширения, кодеки по probe, минимальная и максимальная длительность, мин)
RECORDINGS = {
    "A1": ((".m4a",), {"AAC"}, 55, 65),
    "A2": ((".mp3",), {"MP3"}, 40, 50),
    "A3": ((".m4a",), {"AAC"}, 25, 35),
    "A4": ((".mp3",), {"MP3"}, 30, 40),
    "A5": ((".mp3", ".m4a"), {"MP3", "AAC"}, 25, 35),
    "A6": ((".ogg", ".opus"), {"Opus"}, 1, 5),
}
FRAGMENT_SOURCES = ("A1", "A2", "A3", "A5")
FRAGMENT_SECONDS = (295.0, 305.0)
MINIMAL_RECORDINGS = ("A1", "A4", "A5")
MINIMAL_FRAGMENTS = ("A1", "A5")
TERMS_RANGE = (20, 40)
TRAPS_COUNT = 5

Probe = Callable[[Path], dict]


@dataclass
class Report:
    passed: list[str] = field(default_factory=list)
    failed: list[str] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)

    def ok(self, msg: str) -> None:
        self.passed.append(msg)

    def fail(self, msg: str) -> None:
        self.failed.append(msg)

    @property
    def success(self) -> bool:
        return not self.failed


def engine_probe(path: Path) -> dict:
    proc = subprocess.run([str(whisper_run.ENGINE), "probe", str(path)], capture_output=True, text=True)
    lines = [ln for ln in proc.stdout.splitlines() if ln.strip()]
    return json.loads(lines[-1]) if lines else {"type": "error", "code": "NO_OUTPUT", "message": proc.stderr.strip()}


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def _find_recording(root: Path, rid: str, exts: tuple[str, ...]) -> Path | None:
    for ext in exts:
        p = root / "recordings" / f"{rid}{ext}"
        if p.is_file():
            return p
    return None


def check_recordings(root: Path, ids: tuple[str, ...], probe: Probe, report: Report) -> None:
    for rid in ids:
        exts, codecs, lo, hi = RECORDINGS[rid]
        path = _find_recording(root, rid, exts)
        if path is None:
            report.fail(f"{rid}: нет файла recordings/{rid}{'|'.join(exts)}")
            continue
        info = probe(path)
        if info.get("type") != "probe":
            report.fail(f"{rid}: не читается ({info.get('code')}: {info.get('message')})")
            continue
        minutes = (info.get("duration_ms") or 0) / 60000
        if info.get("codec") not in codecs:
            report.fail(f"{rid}: кодек {info.get('codec')}, нужен {'/'.join(sorted(codecs))}")
        elif not lo <= minutes <= hi:
            report.fail(f"{rid}: длительность {minutes:.1f} мин, нужно {lo}–{hi}")
        else:
            report.ok(f"{rid}: {info['codec']}, {minutes:.1f} мин")


def _read_json(path: Path) -> dict | None:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else None
    except (OSError, ValueError):
        return None


def check_fragments(root: Path, ids: tuple[str, ...], probe: Probe, report: Report) -> dict[str, str]:
    """Возвращает тексты эталонов (id -> текст) для проверки словаря."""
    refs: dict[str, str] = {}
    for rid in ids:
        base = root / "fragments" / f"{rid}f"
        wav, ref, draft, meta = (base.with_suffix(s) for s in (".wav", ".ref.txt", ".draft.txt", ".json"))
        missing = [p.name for p in (wav, ref, draft, meta) if not p.is_file()]
        if missing:
            report.fail(f"{rid}f: нет файлов {', '.join(missing)}")
            continue
        info = probe(wav)
        secs = (info.get("duration_ms") or 0) / 1000
        if info.get("type") != "probe" or not FRAGMENT_SECONDS[0] <= secs <= FRAGMENT_SECONDS[1]:
            report.fail(f"{rid}f: длительность {secs:.0f} с, нужно 295–305")
            continue
        if (info.get("sample_rate"), info.get("channels")) != (16000, 1):
            report.fail(f"{rid}f.wav: {info.get('sample_rate')} Гц, {info.get('channels')} кан.; нужно 16000 Гц, моно")
            continue
        text = ref.read_text(encoding="utf-8")
        if len(normalize(text).split()) < 200:
            report.fail(f"{rid}f: эталон слишком короткий для 5 минут речи (меньше 200 слов)")
            continue
        m = _read_json(meta) or {}
        problems = [k for k in ("source", "start_ms", "draft_model", "reviewer") if k not in m]
        if problems:
            report.fail(f"{rid}f.json: нет полей {', '.join(problems)}")
        elif m.get("source") != rid:
            report.fail(f"{rid}f.json: source = {m.get('source')!r}, ожидалось {rid!r}")
        elif m.get("review2_done") is not True:
            report.fail(f"{rid}f.json: второй проход не отмечен (review2_done: true)")
        elif m.get("draft_model") in (None, "", "large-v3-turbo-q5_0", "large-v3-turbo"):
            report.fail(f"{rid}f.json: черновик не должен делаться моделью-кандидатом (turbo), указано {m.get('draft_model')!r}")
        elif normalize(text) == normalize(draft.read_text(encoding="utf-8")):
            report.fail(f"{rid}f: эталон совпадает с черновиком — ручная правка не выполнена")
        else:
            report.ok(f"{rid}f: {secs:.0f} с, {len(normalize(text).split())} слов, черновик {m['draft_model']}")
            refs[rid] = text
    return refs


def check_glossary(root: Path, refs: dict[str, str], report: Report) -> None:
    path = root / "glossary" / "terms.json"
    data = _read_json(path)
    if data is None:
        report.fail("glossary/terms.json: нет файла или он не JSON-объект")
        return
    items = data.get("terms")
    traps = data.get("traps")
    if not isinstance(items, list) or not isinstance(traps, list):
        report.fail("glossary/terms.json: нужны списки terms и traps")
        return
    terms = [i["term"] if isinstance(i, dict) and "term" in i else i for i in items]
    if not all(isinstance(t, str) and t.strip() for t in terms + traps):
        report.fail("glossary/terms.json: термины должны быть непустыми строками")
        return
    if not TERMS_RANGE[0] <= len(terms) <= TERMS_RANGE[1]:
        report.fail(f"словарь: {len(terms)} терминов, нужно {TERMS_RANGE[0]}–{TERMS_RANGE[1]}")
    if len(traps) != TRAPS_COUNT:
        report.fail(f"словарь: {len(traps)} «ловушек», нужно {TRAPS_COUNT}")
    if not refs:
        report.fail("словарь: нет принятых эталонов, проверить вхождения нельзя")
        return
    words = normalize("\n".join(refs.values())).split()
    absent = [t for t in terms if _count(words, normalize(t).split()) == 0]
    leaked = [t for t in traps if _count(words, normalize(t).split()) > 0]
    if absent:
        report.fail(f"словарь: нет в эталонах (нужно хотя бы одно вхождение): {', '.join(absent)}")
    if leaked:
        report.fail(f"словарь: «ловушки» встречаются в эталонах: {', '.join(leaked)}")
    if not absent and not leaked and len(terms) in range(TERMS_RANGE[0], TERMS_RANGE[1] + 1) and len(traps) == TRAPS_COUNT:
        report.ok(f"словарь: {len(terms)} терминов, каждый есть в эталонах; {len(traps)} ловушек отсутствуют")


def check_manifest(root: Path, report: Report) -> None:
    path = root / "manifest.csv"
    if not path.is_file():
        report.fail("manifest.csv: нет файла")
        return
    with path.open(encoding="utf-8", newline="") as f:
        rows = list(csv.DictReader(f))
    bad = 0
    for row in rows:
        file = root / (row.get("file") or "")
        if not file.is_file():
            report.fail(f"manifest.csv: нет файла {row.get('file')}")
            bad += 1
        elif row.get("kind") == "recording" and not (row.get("consent_date") or "").strip():
            report.fail(f"manifest.csv: {row.get('id')}: не указана дата согласия")
            bad += 1
        elif (row.get("sha256") or "").strip().lower() != _sha256(file):
            report.fail(f"manifest.csv: {row.get('id')}: SHA-256 не совпадает с файлом")
            bad += 1
    if rows and not bad:
        report.ok(f"manifest.csv: {len(rows)} файлов, SHA-256 совпадают, согласия указаны")
    elif not rows:
        report.fail("manifest.csv: пуст")


def check_corpus(root: Path, minimal: bool = False, probe: Probe = engine_probe) -> Report:
    report = Report()
    rec_ids = MINIMAL_RECORDINGS if minimal else tuple(RECORDINGS)
    frag_ids = MINIMAL_FRAGMENTS if minimal else FRAGMENT_SOURCES
    check_recordings(root, rec_ids, probe, report)
    refs = check_fragments(root, frag_ids, probe, report)
    check_glossary(root, refs, report)
    check_manifest(root, report)
    if not minimal:
        report.skipped.append("errors/: файлы для проверки ошибок проверяются вручную (docs/CORPUS_REQUIREMENTS.md §5)")
    return report


def print_report(report: Report) -> None:
    for line in report.passed:
        print(f"  ок       {line}")
    for line in report.failed:
        print(f"  НЕ ОК    {line}")
    for line in report.skipped:
        print(f"  пропущено {line}")
    print("\nКорпус соответствует требованиям." if report.success else f"\nНе выполнено проверок: {len(report.failed)}.")
