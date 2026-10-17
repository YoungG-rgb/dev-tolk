"""WER и CER по нормализованным текстам."""

from __future__ import annotations

from dataclasses import dataclass

from .normalize import normalize


@dataclass(frozen=True)
class WerResult:
    ref_words: int
    substitutions: int
    deletions: int
    insertions: int

    @property
    def errors(self) -> int:
        return self.substitutions + self.deletions + self.insertions

    @property
    def wer(self) -> float:
        return self.errors / self.ref_words if self.ref_words else float(self.errors > 0)


def word_errors(ref: list[str], hyp: list[str]) -> WerResult:
    """Расстояние Левенштейна по словам с разбивкой на замены, удаления и вставки."""
    n, m = len(ref), len(hyp)
    # dist[i][j] — расстояние между ref[:i] и hyp[:j]; для разбивки нужна вся таблица (тексты короткие: фрагменты по 5 мин).
    dist = [[0] * (m + 1) for _ in range(n + 1)]
    for i in range(n + 1):
        dist[i][0] = i
    for j in range(m + 1):
        dist[0][j] = j
    for i in range(1, n + 1):
        ri = ref[i - 1]
        row, prev = dist[i], dist[i - 1]
        for j in range(1, m + 1):
            cost = 0 if ri == hyp[j - 1] else 1
            row[j] = min(prev[j - 1] + cost, prev[j] + 1, row[j - 1] + 1)
    s = d = ins = 0
    i, j = n, m
    while i > 0 or j > 0:
        if i > 0 and j > 0 and dist[i][j] == dist[i - 1][j - 1] + (0 if ref[i - 1] == hyp[j - 1] else 1):
            if ref[i - 1] != hyp[j - 1]:
                s += 1
            i, j = i - 1, j - 1
        elif i > 0 and dist[i][j] == dist[i - 1][j] + 1:
            d += 1
            i -= 1
        else:
            ins += 1
            j -= 1
    return WerResult(n, s, d, ins)


def levenshtein(a: str, b: str) -> int:
    """Расстояние Левенштейна для строк, алгоритм Хиррё (битовые векторы на больших целых): O(len(b)) операций."""
    if not a:
        return len(b)
    if not b:
        return len(a)
    m = len(a)
    mask = (1 << m) - 1
    peq: dict[str, int] = {}
    for i, ch in enumerate(a):
        peq[ch] = peq.get(ch, 0) | (1 << i)
    pv, mv, score = mask, 0, m
    high = 1 << (m - 1)
    for ch in b:
        eq = peq.get(ch, 0)
        xv = eq | mv
        xh = (((eq & pv) + pv) ^ pv) | eq
        ph = mv | ~(xh | pv) & mask
        mh = pv & xh
        if ph & high:
            score += 1
        elif mh & high:
            score -= 1
        ph = ((ph << 1) | 1) & mask
        mh = (mh << 1) & mask
        pv = (mh | ~(xv | ph)) & mask
        mv = ph & xv
    return score


def wer(reference: str, hypothesis: str) -> WerResult:
    return word_errors(normalize(reference).split(), normalize(hypothesis).split())


def cer(reference: str, hypothesis: str) -> float:
    ref, hyp = normalize(reference), normalize(hypothesis)
    if not ref:
        return float(bool(hyp))
    return levenshtein(ref, hyp) / len(ref)
