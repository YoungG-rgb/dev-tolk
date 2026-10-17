"""Точность терминов словаря (BENCHMARK_PLAN §5.2, MVP-037).

- recall: доля вхождений терминов из эталона, найденных в результате в каноническом написании;
- false_insertions: сколько раз термин встретился в результате сверх эталона (включая «ловушки»,
  которых в записи нет). Это сигнал, что подсказка «протекает» в текст.
Сравнение без учёта регистра по нормализованному тексту, термин ищется как последовательность слов.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from .normalize import normalize


@dataclass(frozen=True)
class TermsReport:
    reference_occurrences: int
    found: int
    false_insertions: int

    @property
    def recall(self) -> float:
        return self.found / self.reference_occurrences if self.reference_occurrences else float("nan")


def _count(words: list[str], term: list[str]) -> int:
    n = len(term)
    if n == 0:
        return 0
    return sum(1 for i in range(len(words) - n + 1) if words[i : i + n] == term)


def load_terms(path: Path) -> list[str]:
    """Канонические написания из файла словаря `*.tolk-glossary.json` или из простого JSON-списка строк."""
    data = json.loads(path.read_text(encoding="utf-8"))
    items = data["terms"] if isinstance(data, dict) else data
    return [item["term"] if isinstance(item, dict) else item for item in items]


def term_accuracy(reference: str, hypothesis: str, terms: list[str]) -> TermsReport:
    ref_words, hyp_words = normalize(reference).split(), normalize(hypothesis).split()
    total = found = false_ins = 0
    for term in terms:
        t = normalize(term).split()
        in_ref, in_hyp = _count(ref_words, t), _count(hyp_words, t)
        total += in_ref
        found += min(in_ref, in_hyp)
        false_ins += max(0, in_hyp - in_ref)
    return TermsReport(total, found, false_ins)
