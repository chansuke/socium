# Working in this repository

- Files in `gen/` are generated from `schema/*.json` by `python3 tools/generate.py`.
  Never edit them by hand: change the schema, then run the generator.
- Every schema field needs a `description`.
