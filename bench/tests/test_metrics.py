import random
import unittest

from tolkbench.metrics import cer, levenshtein, wer, word_errors
from tolkbench.normalize import normalize
from tolkbench.repeats import find_repeats
from tolkbench.terms import term_accuracy


def dp_levenshtein(a, b):
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j - 1] + (ca != cb), prev[j] + 1, cur[j - 1] + 1))
        prev = cur
    return prev[-1]


class NormalizeTests(unittest.TestCase):
    def test_case_yo_punctuation(self):
        self.assertEqual(normalize("Всё, ЧТО было — ЕЁ!"), "все что было ее")

    def test_fillers_removed(self):
        self.assertEqual(normalize("Ну, э-э, я думаю... эм... так"), "я думаю так")

    def test_hyphenated_word_split_consistently(self):
        self.assertEqual(normalize("из-за"), normalize("из за"))

    def test_digit_groups_joined(self):
        self.assertEqual(normalize("1 000 рублей и 25 штук"), "1000 рублей и 25 штук")

    def test_empty(self):
        self.assertEqual(normalize("  ...  "), "")


class LevenshteinTests(unittest.TestCase):
    def test_matches_dynamic_programming_on_random_strings(self):
        rng = random.Random(7)
        alphabet = "абвгд "
        for _ in range(300):
            a = "".join(rng.choice(alphabet) for _ in range(rng.randint(0, 40)))
            b = "".join(rng.choice(alphabet) for _ in range(rng.randint(0, 40)))
            self.assertEqual(levenshtein(a, b), dp_levenshtein(a, b), (a, b))

    def test_long_strings_fast_path(self):
        a = "слово " * 2000
        self.assertEqual(levenshtein(a, a), 0)
        self.assertEqual(levenshtein(a, a[:-6]), 6)


class WerTests(unittest.TestCase):
    def test_identical_after_normalization(self):
        self.assertEqual(wer("Привет, мир!", "привет мир").errors, 0)

    def test_counts_substitution_deletion_insertion(self):
        r = word_errors("a b c d".split(), "a x c d e".split())
        self.assertEqual((r.substitutions, r.deletions, r.insertions), (1, 0, 1))
        self.assertAlmostEqual(r.wer, 2 / 4)
        r = word_errors("a b c".split(), "a c".split())
        self.assertEqual((r.substitutions, r.deletions, r.insertions), (0, 1, 0))

    def test_cer(self):
        self.assertAlmostEqual(cer("кот", "кит"), 1 / 3)
        self.assertEqual(cer("", ""), 0.0)


class RepeatsTests(unittest.TestCase):
    def test_loop_detected(self):
        text = "начало " + "спасибо за просмотр друзья " * 5 + "конец"
        r = find_repeats(text)
        self.assertEqual(r.runs, 1)
        self.assertEqual(r.repeated_words, 16)

    def test_normal_speech_has_no_repeats(self):
        self.assertEqual(find_repeats("мы обсудили план и решили начать работу в понедельник утром").runs, 0)

    def test_short_phrase_repeated_twice_is_not_a_loop(self):
        self.assertEqual(find_repeats("да нет да нет да нет").runs, 0)  # n < 4 не считается


class TermsTests(unittest.TestCase):
    def test_recall_and_false_insertions(self):
        ref = "Мы ведём задачи в Jira и деплоим в Kubernetes. Jira удобна."
        hyp = "Мы ведём задачи в Jira и деплоим в кубернетес. Jira удобна. Grafana."
        r = term_accuracy(ref, hyp, ["Jira", "Kubernetes", "Grafana"])
        self.assertEqual((r.reference_occurrences, r.found, r.false_insertions), (3, 2, 1))
        self.assertAlmostEqual(r.recall, 2 / 3)

    def test_multiword_term(self):
        r = term_accuracy("запустили Apache Kafka", "запустили apache kafka", ["Apache Kafka"])
        self.assertEqual((r.reference_occurrences, r.found), (1, 1))


if __name__ == "__main__":
    unittest.main()
