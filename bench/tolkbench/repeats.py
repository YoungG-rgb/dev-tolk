"""Галлюцинации-зацикливания: подряд повторяющиеся n-граммы (BENCHMARK_PLAN §5.2, метрика 1)."""

from __future__ import annotations

from dataclasses import dataclass

from .normalize import normalize

MIN_N = 4
MAX_N = 12
MIN_REPEATS = 3


@dataclass(frozen=True)
class RepeatReport:
    runs: int  # сколько зацикливаний найдено
    repeated_words: int  # сколько слов приходится на повторы сверх первого вхождения


def find_repeats(text: str, min_n: int = MIN_N, max_n: int = MAX_N, min_repeats: int = MIN_REPEATS) -> RepeatReport:
    words = normalize(text).split()
    covered = [False] * len(words)
    runs = repeated = 0
    for n in range(min_n, max_n + 1):
        i = 0
        while i + n * min_repeats <= len(words):
            if covered[i]:
                i += 1
                continue
            gram = words[i : i + n]
            k = 1
            while words[i + k * n : i + (k + 1) * n] == gram:
                k += 1
            if k >= min_repeats:
                runs += 1
                repeated += (k - 1) * n
                for p in range(i, i + k * n):
                    covered[p] = True
                i += k * n
            else:
                i += 1
    return RepeatReport(runs, repeated)
