#!/bin/sh
# Prints a long, noisy log and fails at the end, as many builds do.
i=1
while [ "$i" -le 3000 ]; do
  echo "compiling unit $i of 3000: ok (warnings: 0, notes: 2, cache: hit)"
  i=$((i + 1))
done
echo "src/parse.c:217:14: error: expected ';' after expression" >&2
exit 1
