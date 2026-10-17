import csv
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from tolkbench import corpus

WORDS = " ".join(f"слово{i}" for i in range(250))


def fake_probe(table):
    def probe(path: Path) -> dict:
        key = str(path.relative_to(ROOT[0]))
        return table.get(key, {"type": "error", "code": "FILE_NOT_FOUND", "message": "нет"})
    return probe


ROOT = [Path()]


def ok_info(codec, minutes):
    return {"type": "probe", "codec": codec, "duration_ms": int(minutes * 60000), "sample_rate": 44100, "channels": 1}


def wav_info(seconds=300):
    return {"type": "probe", "codec": "PCM", "duration_ms": int(seconds * 1000), "sample_rate": 16000, "channels": 1}


def build(root: Path, terms=None, traps=None, ref_terms=None, ref_extra="", meta_over=None, with_manifest=True):
    ROOT[0] = root
    (root / "recordings").mkdir()
    (root / "fragments").mkdir()
    (root / "glossary").mkdir()
    files = {"A1.m4a": b"a1", "A4.mp3": b"a4", "A5.m4a": b"a5"}
    for name, data in files.items():
        (root / "recordings" / name).write_bytes(data)
    terms = terms if terms is not None else [f"Терм{i}" for i in range(20)]
    traps = traps if traps is not None else [f"Ловушка{i}" for i in range(5)]
    ref_text = WORDS + " " + " ".join(ref_terms if ref_terms is not None else terms) + " " + ref_extra
    for rid in ("A1", "A5"):
        (root / "fragments" / f"{rid}f.wav").write_bytes(b"w" + rid.encode())
        (root / "fragments" / f"{rid}f.ref.txt").write_text(ref_text, encoding="utf-8")
        (root / "fragments" / f"{rid}f.draft.txt").write_text("совсем другой черновик", encoding="utf-8")
        meta = {"source": rid, "start_ms": 1000, "draft_model": "large-v3-q5_0", "reviewer": "x", "review2_done": True}
        meta.update(meta_over or {})
        (root / "fragments" / f"{rid}f.json").write_text(json.dumps(meta), encoding="utf-8")
    (root / "glossary" / "terms.json").write_text(
        json.dumps({"terms": [{"term": t} for t in terms], "traps": traps}, ensure_ascii=False), encoding="utf-8")
    if with_manifest:
        rows = [("A1", "recordings/A1.m4a", "recording"), ("A4", "recordings/A4.mp3", "recording"),
                ("A5", "recordings/A5.m4a", "recording"), ("A1f", "fragments/A1f.wav", "fragment"),
                ("A5f", "fragments/A5f.wav", "fragment")]
        with (root / "manifest.csv").open("w", encoding="utf-8", newline="") as f:
            w = csv.writer(f)
            w.writerow(["id", "file", "kind", "consent_date", "sha256"])
            for rid, file, kind in rows:
                w.writerow([rid, file, kind, "2026-10-01" if kind == "recording" else "",
                            hashlib.sha256((root / file).read_bytes()).hexdigest()])
    return fake_probe({
        "recordings/A1.m4a": ok_info("AAC", 60), "recordings/A4.mp3": ok_info("MP3", 35),
        "recordings/A5.m4a": ok_info("AAC", 30), "fragments/A1f.wav": wav_info(), "fragments/A5f.wav": wav_info(),
    })


class CorpusTests(unittest.TestCase):
    def check(self, **kw):
        with tempfile.TemporaryDirectory() as d:
            probe = build(Path(d), **{k: v for k, v in kw.items() if k != "probe_patch"})
            return corpus.check_corpus(Path(d), minimal=True, probe=probe)

    def test_valid_minimal_corpus_passes(self):
        r = self.check()
        self.assertTrue(r.success, r.failed)
        self.assertGreaterEqual(len(r.passed), 6)

    def test_term_missing_from_reference_fails(self):
        r = self.check(terms=[f"Терм{i}" for i in range(19)] + ["Отсутствует"], ref_terms=[f"Терм{i}" for i in range(19)])
        self.assertTrue(any("Отсутствует" in m for m in r.failed), r.failed)

    def test_trap_present_in_reference_fails(self):
        r = self.check(ref_extra="Ловушка3")
        self.assertTrue(any("Ловушка3" in m for m in r.failed), r.failed)

    def test_wrong_number_of_terms_and_traps_fails(self):
        r = self.check(terms=[f"Терм{i}" for i in range(10)], traps=["Одна"])
        text = " ".join(r.failed)
        self.assertIn("10 терминов", text)
        self.assertIn("1 «ловушек»", text)

    def test_turbo_draft_is_rejected(self):
        r = self.check(meta_over={"draft_model": "large-v3-turbo-q5_0"})
        self.assertTrue(any("черновик не должен" in m for m in r.failed), r.failed)

    def test_second_review_required(self):
        r = self.check(meta_over={"review2_done": False})
        self.assertTrue(any("второй проход" in m for m in r.failed), r.failed)

    def test_wrong_duration_and_codec_fail(self):
        with tempfile.TemporaryDirectory() as d:
            probe = build(Path(d))
            def patched(path):
                rel = str(path.relative_to(Path(d)))
                if rel.endswith("A1.m4a"):
                    return ok_info("AAC", 20)
                if rel.endswith("A4.mp3"):
                    return ok_info("AAC", 35)
                return probe(path)
            r = corpus.check_corpus(Path(d), minimal=True, probe=patched)
        text = " ".join(r.failed)
        self.assertIn("A1: длительность 20.0 мин", text)
        self.assertIn("A4: кодек AAC", text)

    def test_manifest_hash_mismatch_and_missing_consent(self):
        with tempfile.TemporaryDirectory() as d:
            probe = build(Path(d))
            (Path(d) / "recordings" / "A1.m4a").write_bytes(b"changed")
            r = corpus.check_corpus(Path(d), minimal=True, probe=probe)
        self.assertTrue(any("SHA-256" in m for m in r.failed), r.failed)

    def test_missing_manifest(self):
        r = self.check(with_manifest=False)
        self.assertTrue(any("manifest.csv" in m for m in r.failed), r.failed)

    def test_full_mode_requires_all_recordings(self):
        with tempfile.TemporaryDirectory() as d:
            probe = build(Path(d))
            r = corpus.check_corpus(Path(d), minimal=False, probe=probe)
        text = " ".join(r.failed)
        for rid in ("A2", "A3", "A6"):
            self.assertIn(f"{rid}: нет файла", text)


if __name__ == "__main__":
    unittest.main()
