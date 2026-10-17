"""Командная строка: python3 -m tolkbench <команда> (запускать из каталога bench/)."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

from . import corpus, metrics, prepare, repeats, sps, terms, whisper_run


def cmd_score(args: argparse.Namespace) -> None:
    hyp = Path(args.hyp).read_text(encoding="utf-8")
    out: dict = {"repeats": repeats.find_repeats(hyp).runs}
    if args.ref:
        ref = Path(args.ref).read_text(encoding="utf-8")
        w = metrics.wer(ref, hyp)
        out |= {"wer": round(w.wer, 4), "cer": round(metrics.cer(ref, hyp), 4),
                "substitutions": w.substitutions, "deletions": w.deletions, "insertions": w.insertions,
                "ref_words": w.ref_words}
        if args.glossary:
            t = terms.term_accuracy(ref, hyp, terms.load_terms(Path(args.glossary)))
            out |= {"term_recall": round(t.recall, 4), "term_false_insertions": t.false_insertions}
    print(json.dumps(out, ensure_ascii=False))


def cmd_whisper(args: argparse.Namespace) -> None:
    glossary = terms.load_terms(Path(args.glossary)) if args.glossary else None
    row = whisper_run.run(args.config, Path(args.audio), args.machine, args.record, args.threads, glossary, args.dry_run)
    if args.dry_run:
        return
    transcript = Path(row.pop("_transcript"))
    print(f"Транскрипт: {transcript}. Оценка по эталону — командой `score` (эталон есть только для фрагментов).",
          file=sys.stderr)
    whisper_run.append_row(Path(args.results), row)
    print(json.dumps(row, ensure_ascii=False))


def cmd_hashes(args: argparse.Namespace) -> None:
    for path in sorted(Path(args.dir).iterdir()):
        if path.is_file() and not path.name.startswith("."):
            h = hashlib.sha256()
            with path.open("rb") as f:
                for block in iter(lambda: f.read(1 << 20), b""):
                    h.update(block)
            print(f"{h.hexdigest()}  {path.name}")


def cmd_check_corpus(args: argparse.Namespace) -> None:
    report = corpus.check_corpus(Path(args.dir), minimal=args.minimal)
    corpus.print_report(report)
    sys.exit(0 if report.success else 1)


def cmd_prepare_fragment(args: argparse.Namespace) -> None:
    result = prepare.prepare_fragment(Path(args.dir), args.id, prepare.parse_time(args.start), args.reviewer, args.force, args.threads)
    print(f"Фрагмент: {result['wav']} ({result['seconds']:.0f} с из записи {result['recording_seconds'] / 60:.1f} мин)")
    print(f"Черновик (large-v3-q5_0): {result['draft']}")
    if result["ref_created_from_draft"]:
        print(f"Заготовка эталона: {result['ref']} — скопирована из черновика. ВАША РАБОТА: прослушать фрагмент и исправить каждое слово.")
    else:
        print(f"Эталон {result['ref']} уже есть и не изменён.")
    print(f"Метаданные: {result['meta']} (review2_done=false: после второго прослушивания поставьте true)")


def cmd_manifest(args: argparse.Namespace) -> None:
    rows = prepare.write_manifest(Path(args.dir), args.consent_date or "")
    missing = [r["id"] for r in rows if r["kind"] == "recording" and not r["consent_date"]]
    print(f"manifest.csv: {len(rows)} файлов, SHA-256 и длительность обновлены.")
    if missing:
        print(f"Не указана дата согласия: {', '.join(missing)} (колонка consent_date или --consent-date ГГГГ-ММ-ДД).")


def cmd_build_sps(args: argparse.Namespace) -> None:
    r = sps.build(Path(args.src), Path(args.dest), args.minutes, args.speaker)
    print(f"{r['wav']}: {r['clips']} клипов, {r['seconds'] / 60:.1f} мин, диктор {r['speaker']}…, {r['words']} слов в эталоне")
    print(f"Эталон: {r['ref']}")


def main() -> None:
    ap = argparse.ArgumentParser(prog="tolkbench")
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("build-sps", help="склеить клипы Common Voice Spontaneous Speech одного диктора в длинную запись с эталоном")
    p.add_argument("src", help="каталог распакованного sps-corpus (с ss-corpus-ru.tsv и audios/)")
    p.add_argument("dest", help="куда положить S1.wav и S1.ref.txt (вне репозитория)")
    p.add_argument("--minutes", type=float, default=45)
    p.add_argument("--speaker", help="client_id диктора (по умолчанию — с наибольшей суммарной длительностью)")
    p.set_defaults(fn=cmd_build_sps)

    p = sub.add_parser("score", help="WER/CER, повторы и точность терминов для результата")
    p.add_argument("hyp", help="текст результата")
    p.add_argument("--ref", help="эталон (для WER/CER)")
    p.add_argument("--glossary", help="словарь терминов (JSON)")
    p.set_defaults(fn=cmd_score)

    p = sub.add_parser("whisper", help="запуск whisper-cli с замером (macOS)")
    p.add_argument("config", choices=sorted(whisper_run.CONFIGS))
    p.add_argument("audio")
    p.add_argument("--machine", required=True, help="H1, H2, ...")
    p.add_argument("--record", required=True, help="A1, A2, ...")
    p.add_argument("--threads", type=int, default=4)
    p.add_argument("--glossary")
    p.add_argument("--results", default=str(Path(__file__).resolve().parents[2] / "docs" / "bench" / "results.csv"))
    p.add_argument("--dry-run", action="store_true", help="только показать команду")
    p.set_defaults(fn=cmd_whisper)

    p = sub.add_parser("hashes", help="SHA-256 файлов каталога корпуса (MVP-002)")
    p.add_argument("dir")
    p.set_defaults(fn=cmd_hashes)

    p = sub.add_parser("check-corpus", help="проверка корпуса по docs/CORPUS_REQUIREMENTS.md")
    p.add_argument("dir")
    p.add_argument("--minimal", action="store_true", help="только минимальный набор для первого бенчмарка")
    p.set_defaults(fn=cmd_check_corpus)

    p = sub.add_parser("prepare-fragment", help="нарезать 5-минутный фрагмент и сделать черновик эталона (large-v3-q5_0)")
    p.add_argument("dir")
    p.add_argument("id", choices=list(corpus.FRAGMENT_SOURCES))
    p.add_argument("--start", required=True, help="начало фрагмента: секунды, мм:сс или чч:мм:сс")
    p.add_argument("--reviewer", default="")
    p.add_argument("--threads", type=int, default=4)
    p.add_argument("--force", action="store_true")
    p.set_defaults(fn=cmd_prepare_fragment)

    p = sub.add_parser("manifest", help="заполнить manifest.csv по файлам корпуса (SHA-256, длительность, кодек)")
    p.add_argument("dir")
    p.add_argument("--consent-date", help="дата согласия участников (указывает человек), если у всех записей одна")
    p.set_defaults(fn=cmd_manifest)

    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
