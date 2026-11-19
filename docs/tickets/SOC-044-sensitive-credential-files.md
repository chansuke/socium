# SOC-044: よくある認証情報ファイルを読むときに承認を求める

GitHub: なし (2026-11-19 の公開前レビューで発見)

## 問題
既定の `approval.sensitive` は `.env`、`.netrc`、鍵ファイルなどを守るが、Composer の `auth.json`、Google の `credentials.json` と `client_secret*.json`、PostgreSQL の `.pgpass` は守っていなかった。リポジトリに書かれた指示でモデルがこれらを読むと、内容はそのままプロバイダへ送られる。

## 対応・受け入れ条件
- 上の 4 種を既定の `approval.sensitive` に入れる。ファイルツールでの読み取りは承認を求め、確認していないコマンドからは空のファイルに見せる。
- `auth.json` はどの階層でも対象にする。Composer のプロジェクトはサブディレクトリにもある。翻訳ファイルの `auth.json` も読むときに承認を求めるが、承認はそのセッションのあいだ有効。
- シンボリックリンクは、リンク先のパスと、リンクそのもののパスの両方で照合する。`credentials.json` が別名のファイルを指していても承認を求める。

## 完了記録

2026-11-19 完了。

- 試験: `crates/core/src/settings.rs` で各パターンが入れ子の `backend/auth.json` にも一致することを確かめる。`crates/core/src/approval.rs` で、`auth.json` の読み取りと、別のファイルを指す `credentials.json` リンクの読み取りが承認を求めること、確認していないコマンドから `backend/auth.json` が隠れることを確かめる。
- ホームの `~/.codex/auth.json` などは元からサンドボックスの外にあり、コマンドからは見えない。
