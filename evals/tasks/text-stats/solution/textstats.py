import re
from collections import Counter


def words(text):
    runs = (run.strip("'").lower() for run in re.findall(r"[A-Za-z']+", text))
    return [run for run in runs if run]


def sentences(text):
    return [part for part in re.split(r"[.!?]+(?=\s|$)", text) if words(part)]


def average_sentence_length(text):
    parts = sentences(text)
    if not parts:
        return 0.0
    return round(len(words(text)) / len(parts), 2)


def top_words(text, n=3):
    counts = Counter(words(text))
    return [word for word, _ in sorted(counts.items(), key=lambda item: (-item[1], item[0]))[:n]]
