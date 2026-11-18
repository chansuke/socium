# textstats

- `words(text)`: the maximal runs of ASCII letters and apostrophes, lowercased,
  with apostrophes at either end of a run removed. Runs left empty are dropped.
  `"Don't 'tis"` gives `["don't", "tis"]`.
- `sentences(text)`: the text split after `.`, `!`, or `?` that is followed
  by whitespace or the end of the text. A run of these marks, as in `?!` or
  `...`, ends one sentence. Pieces with no words are dropped. `3.5` does not
  end a sentence.
- `average_sentence_length(text)`: the number of words divided by the number
  of sentences, rounded to 2 places, or `0.0` when there are no sentences.
- `top_words(text, n=3)`: the `n` most frequent words, most frequent first;
  words with the same count in alphabetical order.
