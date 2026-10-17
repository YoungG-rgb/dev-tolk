"""Нормализация текста перед сравнением (BENCHMARK_PLAN §2.2). Одинакова для эталона и для всех систем.

Правила:
- нижний регистр;
- ё -> е;
- вся пунктуация удаляется (дефис между буквами внутри слова заменяется пробелом: «из-за» -> «из за»,
  одинаково в эталоне и в результате, поэтому сравнение остаётся честным);
- пробелы схлопываются;
- числа не конвертируются: остаются как записаны. Группы цифр, разделённые пробелом («1 000»), склеиваются.
  Эталон должен быть записан в том же виде, что обычно выдаёт модель (цифрами для чисел);
- междометия и слова-паразиты из списка FILLERS удаляются из обоих текстов.
"""

from __future__ import annotations

import re

# Фиксируется в отчёте вместе с результатами. Изменение списка делает прежние результаты несравнимыми.
FILLERS = frozenset({"э", "ээ", "эээ", "эм", "мм", "ммм", "хм", "ну"})

_FILLER_HYPHEN = re.compile(r"\b[эм](?:-[эм])+\b")
_DIGIT_GROUPS = re.compile(r"(?<=\d)[\s  ](?=\d{3}\b)")
_NON_WORD = re.compile(r"[^\w\s]|_", re.UNICODE)
_SPACES = re.compile(r"\s+")


def normalize(text: str, fillers: frozenset[str] = FILLERS) -> str:
    text = text.lower().replace("ё", "е")
    text = _FILLER_HYPHEN.sub(" ", text)
    text = _DIGIT_GROUPS.sub("", text)
    text = _NON_WORD.sub(" ", text)
    words = [w for w in _SPACES.split(text.strip()) if w and w not in fillers]
    return " ".join(words)
