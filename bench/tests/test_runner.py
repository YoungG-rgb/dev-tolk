import unittest
from pathlib import Path

from tolkbench import whisper_run

SAMPLE = """
        2.00 real         1.50 user         0.30 sys
           812056576  maximum resident set size
                   0  average shared memory size
          2123456789  peak memory footprint
"""


class RunnerTests(unittest.TestCase):
    def test_parse_time_l(self):
        t = whisper_run.parse_time_l(SAMPLE)
        self.assertEqual((t.wall_s, t.max_rss_bytes, t.peak_footprint_bytes), (2.0, 812056576, 2123456789))

    def test_parse_time_l_rejects_garbage(self):
        with self.assertRaises(ValueError):
            whisper_run.parse_time_l("нет данных")

    def test_w1_command_matches_plan(self):
        cmd = whisper_run.build_command(whisper_run.CONFIGS["W1"], Path("a.wav"), Path("out"), 4, None)
        for flag in ("-l", "ru", "-mc", "0", "--vad", "-vm"):
            self.assertIn(flag, cmd)
        self.assertNotIn("-ng", cmd)
        self.assertNotIn("-nt", cmd, "--no-timestamps меняет распознавание и теряет слова")
        self.assertIn("ggml-large-v3-turbo-q5_0.bin", cmd[cmd.index("-m") + 1])

    def test_w2_has_no_vad_and_w4_disables_gpu(self):
        self.assertNotIn("--vad", whisper_run.build_command(whisper_run.CONFIGS["W2"], Path("a"), Path("o"), 4, None))
        self.assertIn("-ng", whisper_run.build_command(whisper_run.CONFIGS["W4"], Path("a"), Path("o"), 4, None))

    def test_w9_prompt(self):
        cmd = whisper_run.build_command(whisper_run.CONFIGS["W9"], Path("a"), Path("o"), 4,
                                        whisper_run.glossary_prompt(["Jira", "Kubernetes"]))
        self.assertEqual(cmd[cmd.index("--prompt") + 1], "Термины: Jira, Kubernetes.")
        self.assertIn("--carry-initial-prompt", cmd)
        with self.assertRaises(SystemExit):
            whisper_run.build_command(whisper_run.CONFIGS["W9"], Path("a"), Path("o"), 4, None)


if __name__ == "__main__":
    unittest.main()
