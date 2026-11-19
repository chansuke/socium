# SOC-050: パッケージに依存 crate のライセンス表示を入れる

GitHub: なし (2026-11-19 の公開前レビューで発見)

## 問題
main への push で CI がパッケージを Cachix に置くので、バイナリを配っていることになる。MIT と Apache-2.0 は、配る複製にライセンス表示を付けることを求めるが、パッケージには Socium 自身の `LICENSE` も依存 crate のライセンスも入っていなかった。

## 対応・受け入れ条件
- `share/doc/socium/LICENSE` に Socium のライセンスを入れる。
- `share/doc/socium/THIRD-PARTY-LICENSES` に、ビルドに使う全 crate のライセンスファイルと NOTICE を入れる。crate 直下、REUSE の `LICENSES/`、`license-file` で指したファイルを読む。ライセンスファイルのない crate (NOTICE だけの crate も含む) は、作者と、ライセンスの式にある各ライセンスと例外の SPDX の本文 (nixpkgs の `spdx-license-list-data`) を入れる。`GPL-2.0+` のような `+` は `-or-later` として探す。本文が見つからない crate があればビルドを止める。
- 一覧は Cargo.lock から crane が取ってくる crate のソースから、`nix/third-party-licenses.py` がビルドのたびに作る。手で更新しない。

## 完了記録

2026-11-19 完了。

- 確認: `nix build .#default` で、346 crate の見出しを持つ 3.1 MB のファイルができた。ライセンスファイルのない 12 crate には SPDX の本文を 22 件入れた。この 12 crate は `cargo tree --target x86_64-unknown-linux-gnu` で見ると、どれも Linux のバイナリに入らない。パッケージの closure は 165 MiB から 168 MiB になり、Python は実行時の closure に入らない。
- 試験: `nix/test_third_party_licenses.py` をパッケージのビルドで先に実行する。ライセンスファイルの複写、`LICENSES/` と `license-file`、NOTICE だけの crate、`/`・`+`・`WITH` を含む式、SPDX にないライセンスと式のない crate のエラーを確かめる。
- 一覧には Linux のバイナリに入らない crate と dev-dependency も含む。足りないより多い方を選んだ。
- Nix の closure に入る C ライブラリ (OpenSSL、bubblewrap など) の表示は nixpkgs のパッケージ側にあり、ここでは扱わない。
