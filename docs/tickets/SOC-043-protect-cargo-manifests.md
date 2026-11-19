# SOC-043: Cargo.toml と Cargo.lock の編集に承認を求める

GitHub: なし (2026-11-19 の公開前レビューで発見)

## 問題
`build.rs` と `.cargo/config.toml` の編集は承認を求めるが、`Cargo.toml` と `Cargo.lock` は承認なしで編集できた。モデルが依存を足したり、proc macro を持つ path crate を足したりすると、ホストの rust-analyzer がその build script と proc macro をサンドボックスの外で実行する。[脅威モデル](../threat-model.md)は proc macro を守ると書いていた。

## 対応・受け入れ条件
- `Cargo.toml` を既定の `approval.protected` に入れる。どの階層の `Cargo.toml` も対象にする。
- 誰も確認していないコマンドからは、`Cargo.toml` を読み取り専用にする。
- シンボリックリンクは、リンク先のパスとリンクそのもののパスの両方で照合する。別名のファイルを指す `Cargo.toml` リンクの編集も承認を求める。
- `Cargo.lock` も対象にする。git の依存は lock がコミットを指し、cargo はハッシュで指されたコミットを、fork に push されたものも含めて取ってくる。誰も確認していないコマンドからは読み取り専用になるので、依存を足した後の lock の更新は承認したコマンドで行う。

## 完了記録

2026-11-19 完了。

- 試験: `crates/core/src/approval.rs` で、ルートと `crates/x/` の `Cargo.toml`、`manifest.toml` を指す `Cargo.toml` リンク、`Cargo.lock` の編集が自動実行モードでも承認を求めることと、確認していないコマンドで `Cargo.toml` が読み取り専用になることを確かめる。
- 残る穴は[脅威モデル](../threat-model.md)の「守らないもの」に書いた。ワークスペース内の proc-macro crate のソースと、`Cargo.toml` が指す別名の build script の編集、確認していないコマンドが新しく作る `Cargo.toml`・`build.rs`、保護する名前のリンクが指すファイルへのコマンドの書き込みは承認を求めない。
- `cargo vendor` などで `Cargo.toml` が 256 を超える workspace では、自動実行のコマンドも承認を求めるようになる (既存の上限による安全側の動作)。
