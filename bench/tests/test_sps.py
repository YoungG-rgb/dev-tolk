import unittest

from tolkbench import sps
from tolkbench.normalize import normalize


def row(cid, audio_id, text, ms=1000, votes="1", tags=""):
    return {"client_id": cid, "audio_id": str(audio_id), "transcription": text, "duration_ms": str(ms), "votes": votes,
            "quality_tags": tags, "audio_file": f"a{audio_id}.mp3"}


class PickClipsTest(unittest.TestCase):
    def test_picks_longest_speaker_and_skips_bad_clips(self):
        rows = [
            row("A", 1, "один", 60000),
            row("A", 2, "два [unclear]", 60000),
            row("A", 3, "три", 60000, tags="transcription-length"),
            row("A", 4, "четыре", 60000, votes=""),
            row("A", 5, "пять", 60000),
            row("B", 6, "шесть", 60000),
        ]
        speaker, chosen = sps.pick_clips(rows, None, 10)
        self.assertEqual(speaker, "A")
        self.assertEqual([r["audio_id"] for r in chosen], ["1", "5"])

    def test_limits_total_duration_and_keeps_order(self):
        rows = [row("A", i, "слово", 60000) for i in (3, 1, 2)]
        _, chosen = sps.pick_clips(rows, "A", 2)
        self.assertEqual([r["audio_id"] for r in chosen], ["1", "2"])


class CleanReferenceTest(unittest.TestCase):
    def test_removes_stretched_fillers_only(self):
        self.assertEqual(normalize(sps.clean_reference("Тогда, а-а-а, потом и-и-и... пошли")), "тогда потом пошли")
        self.assertEqual(sps.clean_reference("кто-то из-за угла"), "кто-то из-за угла")


if __name__ == "__main__":
    unittest.main()
