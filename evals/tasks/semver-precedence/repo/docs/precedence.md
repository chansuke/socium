# Version precedence

Versions look like `MAJOR.MINOR.PATCH`, optionally followed by `-` and
dot-separated pre-release identifiers, and then by `+` and build metadata.

1. Compare MAJOR, MINOR, and PATCH as numbers, in that order.
2. When they are equal, a version with pre-release identifiers is lower than
   the same version without any.
3. Two pre-releases are compared one identifier at a time, from the left:
   - identifiers made only of digits are compared as numbers;
   - other identifiers are compared character by character in ASCII order;
   - an identifier made only of digits is lower than one that is not.
   If every identifier they share is equal, the one with more identifiers is
   higher.
4. Build metadata never affects precedence: `1.0.0+a` and `1.0.0+b` compare
   equal.

`latest` returns the highest version as given. Among versions of equal
precedence it returns the first one. An empty input is a `ValueError`.
