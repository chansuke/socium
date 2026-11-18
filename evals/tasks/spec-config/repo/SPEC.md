# conf format

`confparse.parse(text)` reads this format and returns a dict of sections,
each a dict of keys to string values. Syntax errors raise
`confparse.ParseError` with `.line` set to the 1-based line number.

1. Leading and trailing whitespace on each line is ignored. Blank lines and
   lines whose first character is `#` or `;` are comments.
2. `[name]` starts a section. Names are case-insensitive and stored in
   lowercase, with whitespace inside the brackets trimmed. A header repeated
   later continues the same section. Keys before the first header belong to
   the section `global`. A line starting with `[` that does not end with `]`
   is an error.
3. `key = value` sets a key. The line splits at the first `=`; the key is
   trimmed and lowercased and must not be empty; the value is trimmed. A
   line that is not a comment, header, or key is an error.
4. A value starting with `"` is quoted: the quotes are removed and the
   escapes `\"`, `\\`, `\n`, and `\t` are replaced; any other backslash
   escape is an error. Spaces inside quotes are kept. After the closing
   quote only whitespace or a comment (`#` or `;`) may follow. A missing
   closing quote is an error.
5. In an unquoted value, ` #` or ` ;` (whitespace, then `#` or `;`) starts a
   comment that runs to the end of the line.
6. An unquoted value ending with `\` continues on the next line: the
   backslash and any whitespace before it are removed, the next line is
   trimmed, and the two are joined with one space. A continuation line is
   never a comment line or a header, even if it starts with `#` or `[`.
7. A key set twice in one section is an error on the second line.
8. After the whole text is read, unquoted values are interpolated:
   `${key}` is the value of `key` in the same section and `${section.key}`
   the value in another section, itself interpolated first. `$$` is a
   literal `$`. A reference to a missing key, or a cycle of references, is
   an error on the line of the key whose value contains it. Quoted values
   are never interpolated.
