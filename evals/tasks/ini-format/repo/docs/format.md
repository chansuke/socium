# Settings format

`parse(text)` returns a dict of sections, each a dict of string keys to
string values. The section `""` always exists and holds keys that come
before the first section header.

1. Lines are separated by `\n`. A `\r` at the end of a line is removed.
2. A line that is empty, only spaces and tabs, or whose first character
   other than spaces and tabs is `;` or `#` is ignored. Such a line also ends
   any value being continued (rule 7).
3. A line that, trimmed, starts with `[` and ends with `]` is a section
   header. The name between the brackets is trimmed. Later keys go to that
   section. A section named again is the same section: its keys are merged.
4. A line containing `=` is a key line. The key is the text before the first
   `=`, trimmed and lowercased; an empty key is an error. The value is the
   text after the first `=`, trimmed, then read by rule 5 or 6. A later key
   replaces an earlier one with the same name in the same section.
5. An unquoted value ends before the first `;` or `#` that is at its start
   or follows a space or tab; the rest of the line is a comment and the value
   is trimmed again. A `;` or `#` inside a word, as in `a;b`, is kept.
6. A value that starts with `"` is quoted. It ends at the next `"` that is
   not escaped. Inside it, `\"` stands for `"` and `\\` for `\`; any other
   character, including a lone `\`, `;`, `#`, and spaces, is kept as is.
   After the closing quote only spaces, tabs, or a comment (`;` or `#` and
   anything after it) may follow; anything else is an error, as is a quote
   that is never closed.
7. A line that starts with a space or tab, and is not ignored by rule 2,
   continues the value of the key line just before it (or of the
   continuation line before it). Its trimmed text, read by rule 5, is added
   to the value, after one space if the value is not empty. Continuing a
   quoted value, or a line that starts with a space or tab with no value to
   continue, is an error.
8. Any other line is an error.

Every error is a `ValueError` whose message starts with `line N:`, where N
counts lines from 1.
