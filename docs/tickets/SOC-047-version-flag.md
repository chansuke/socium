# SOC-047: `socium --version` で版を表示する

GitHub: なし (2026-11-19 の公開前レビューで発見)

## 問題
`--version` がなく、Nix と Cargo で入れた複数の `socium` が混ざったとき、どれを実行しているかを確かめられなかった。ベンチマークも `--version` の代わりに `--help` を測っていた。

## 対応・受け入れ条件
- `-V`、`--version` で `socium <版>` を表示して終わる。他のフラグと一緒に渡されても版を表示する。
- `--help` に載せる。

## 完了記録

2026-11-19 完了。

- 試験: `crates/tui/src/cli.rs` の `version_wins_over_other_flags` と、`crates/tui/tests/headless.rs` の `version_prints_the_package_version` (バイナリを実行して出力を確かめる)。
- `bench/agents.py` は Socium にも `--version` を使う。
