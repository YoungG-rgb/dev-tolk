import csv
import tempfile
import unittest
import wave
from pathlib import Path

from tolkbench import prepare


def write_ramp(path: Path, seconds: int, rate: int = 16000) -> None:
    """WAV, в котором значение отсчёта равно его номеру (младшие 15 бит): по нему видно, что вырезано."""
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"".join(((i % 30000)).to_bytes(2, "little", signed=True) for i in range(seconds * rate)))


class TimeTests(unittest.TestCase):
    def test_formats(self):
        self.assertEqual(prepare.parse_time("1200"), 1200)
        self.assertEqual(prepare.parse_time("20:00"), 1200)
        self.assertEqual(prepare.parse_time("1:05:30"), 3930)
        self.assertEqual(prepare.parse_time("0:12.5"), 12.5)

    def test_garbage(self):
        for bad in ("", "abc", "1:2:3:4", "-5", "10:xx"):
            with self.assertRaises(ValueError, msg=bad):
                prepare.parse_time(bad)


class SliceTests(unittest.TestCase):
    def test_slice_has_exact_length_and_content(self):
        with tempfile.TemporaryDirectory() as d:
            src, dst = Path(d) / "a.wav", Path(d) / "out" / "f.wav"
            write_ramp(src, 20)
            seconds = prepare.slice_wav(src, dst, start_s=5, duration_s=7)
            self.assertEqual(seconds, 7)
            with wave.open(str(dst), "rb") as w:
                self.assertEqual((w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()), (1, 2, 16000, 7 * 16000))
                first = int.from_bytes(w.readframes(1), "little", signed=True)
            self.assertEqual(first, (5 * 16000) % 30000)
            self.assertEqual([p.name for p in dst.parent.iterdir()], ["f.wav"], "временных файлов не осталось")

    def test_out_of_range_is_rejected_and_nothing_is_written(self):
        with tempfile.TemporaryDirectory() as d:
            src, dst = Path(d) / "a.wav", Path(d) / "f.wav"
            write_ramp(src, 10)
            for start, dur in ((10, 1), (50, 1), (5, 6)):
                with self.assertRaises(ValueError):
                    prepare.slice_wav(src, dst, start, dur)
            self.assertFalse(dst.exists())


class ManifestTests(unittest.TestCase):
    def make_corpus(self, d: Path) -> None:
        (d / "recordings").mkdir()
        (d / "fragments").mkdir()
        (d / "recordings" / "A1.m4a").write_bytes(b"a1")
        (d / "recordings" / "A4.mp3").write_bytes(b"a4")
        write_ramp(d / "fragments" / "A1f.wav", 1)

    @staticmethod
    def probe(path: Path) -> dict:
        return {"type": "probe", "container": "isomp4", "codec": "AAC", "duration_ms": 3_600_000}

    def test_hashes_durations_and_codecs_are_filled_but_consent_is_never_invented(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.make_corpus(root)
            rows = prepare.build_manifest(root, [], probe=self.probe)
            self.assertEqual([r["id"] for r in rows], ["A1", "A4", "A1f"])
            a1 = rows[0]
            self.assertEqual((a1["file"], a1["kind"], a1["duration_s"], a1["container_codec"]), ("recordings/A1.m4a", "recording", "3600", "isomp4/AAC"))
            self.assertEqual(len(a1["sha256"]), 64)
            self.assertTrue(all(r["consent_date"] == "" for r in rows), "дату согласия указывает человек")

    def test_consent_date_flag_applies_to_recordings_only_and_keeps_existing_values(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.make_corpus(root)
            existing = [{"id": "A1", "consent_date": "2026-09-01", "speakers": "5", "notes": "переговорная"}]
            rows = prepare.build_manifest(root, existing, probe=self.probe, consent_date="2026-10-18")
            by = {r["id"]: r for r in rows}
            self.assertEqual(by["A1"]["consent_date"], "2026-09-01", "заполненное человеком не перезаписывается")
            self.assertEqual((by["A1"]["speakers"], by["A1"]["notes"]), ("5", "переговорная"))
            self.assertEqual(by["A4"]["consent_date"], "2026-10-18")
            self.assertEqual(by["A1f"]["consent_date"], "", "у фрагментов согласие не нужно отдельно")

    def test_write_manifest_roundtrip_passes_manifest_check_after_hash_update(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            self.make_corpus(root)
            original = prepare.build_manifest
            prepare.build_manifest = lambda r, e, probe=self.probe, consent_date="": original(r, e, probe=self.probe, consent_date=consent_date)
            try:
                prepare.write_manifest(root, consent_date="2026-10-18")
            finally:
                prepare.build_manifest = original
            with (root / "manifest.csv").open(encoding="utf-8", newline="") as f:
                rows = list(csv.DictReader(f))
            self.assertEqual(len(rows), 3)
            from tolkbench import corpus
            report = corpus.Report()
            corpus.check_manifest(root, report)
            self.assertTrue(report.success, report.failed)


if __name__ == "__main__":
    unittest.main()
