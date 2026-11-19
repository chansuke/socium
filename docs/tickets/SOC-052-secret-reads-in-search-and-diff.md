# SOC-052: 検索と差分からも認証情報ファイルの中身を出さない

GitHub: なし (2026-11-19 の公開前の再レビューで発見)

## 問題
認証情報ファイル (`approval.sensitive`) の読み取りに承認を求めるのは `read_file` と text editor の `view` だけだった。`search_text` に `.env` を path として渡すと中身の行が返り、`git_diff` は追跡している `.env` の変更をそのまま返した。どちらも承認なしでプロバイダへ届く。広い検索で `.env` が出なかったのは ripgrep が隠しファイルを飛ばすからで、隠しファイルでない `credentials.json` は出ていた。

## 対応・受け入れ条件
- `search_text` と `git_diff` の path が認証情報ファイルを指すときは、`read_file` と同じく承認を求める。
- 広い検索と差分 (path なし、またはディレクトリ) は、認証情報ファイルの行と変更を出さず、出さなかったファイルの名前を書く。名前を path にすれば承認を求めて読める。
- `find_symbol` は `*.rs` だけを探し、どの既定のパターンにも当たらないので変えない。

## 完了記録

2026-11-19 完了。

- 試験: `crates/core/src/approval.rs` で、`.env` を指す `search_text` と `git_diff` が承認を求めること。`crates/core/src/tools/search_text.rs` の `a_wide_search_leaves_out_files_that_may_hold_credentials` と `crates/core/src/tools/git.rs` の `git_diff_leaves_out_files_that_may_hold_credentials` で、広い検索と差分が中身を出さずに名前を書き、名前を指したときは出すこと。
- `git` は `--literal-pathspecs` で動かしているので、除外の pathspec は使えない。差分は、変更されたファイルのうち出すものを 1 つずつ指定して取る。
