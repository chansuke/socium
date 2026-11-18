import re
from collections import Counter


def words(text):
    return re.findall(r"[a-z']+", text)


def sentences(text):
    return [part for part in re.split(r"[.!?]", text) if part.strip()]


def average_sentence_length(text):
    return round(len(words(text)) / len(sentences(text)), 2)


def top_words(text, n=3):
    return [word for word, _ in Counter(words(text)).most_common(n)]
