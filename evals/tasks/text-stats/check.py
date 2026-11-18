import sys

sys.path.insert(0, ".")
from textstats import average_sentence_length, sentences, top_words, words

assert words("Don't 'tis O'Neil's '' x-ray") == ["don't", "tis", "o'neil's", "x", "ray"], words("Don't 'tis O'Neil's '' x-ray")
text = "Version 3.5 is out. Really?! Yes... it is"
assert len(sentences(text)) == 4, sentences(text)
assert sentences("No end") == ["No end"]
assert sentences("... !") == []
assert average_sentence_length(text) == 1.75, average_sentence_length(text)
assert average_sentence_length("") == 0.0
assert average_sentence_length("3.5 ?") == 0.0
assert top_words("b a c b a d", 3) == ["a", "b", "c"], top_words("b a c b a d", 3)
assert top_words("Zed zed Ant ant bee", 2) == ["ant", "zed"]
assert top_words("", 2) == []
