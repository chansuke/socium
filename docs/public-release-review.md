# 公開準備・セキュリティ確認

確認日: 2026-11-11。基準commit: `a392ec7`（PR #94）。Linux上でのコードレビュー、ローカルfixture、依存関係・秘密情報の検査を実施した。公開設定の変更や実モデルAPIへの送信は行っていない。

## 整理したもの

- 未使用の `workspace.rs.bak`、実行時に失敗するだけのCodex/Opencode provider雛形、未使用の定数を削除。未対応providerは起動時に拒否する。
- 旧改善計画とTODOを、元のtask IDを保持したSOCチケットへの対応表へ置換。現在の機能・制約と将来計画を区別した。
- 公開用の[文書index](index.md)、[開発手順](../CONTRIBUTING.md)、API設定例、README、セキュリティポリシーを整備。
- レビュー原本と再現証拠は過去の記録として保持。ローカル研究用checkout・草稿、Nix result、環境変数ファイルを公開対象から除外し、既存ローカルファイルは削除していない。

## 発見と修正

| 項目 | 確認した問題 | 修正と検証 |
|---|---|---|
| 依存関係 | Cargo.lockにRustSecの脆弱性22件と警告6件。TLS/HTTP等を含む。各advisoryの存在は、全経路がこのアプリから悪用可能であることを意味しない。 | Rust 1.92互換の範囲でlockfileを更新。更新後は脆弱性0件・警告0件。例外指定なし。 |
| 状態保存のリンク・権限 | 状態ファイルの読込がリンクを追跡し、編集履歴の保存先はworkspace内の別ファイルへ解決できた。旧ファイルの広い権限も引き継いでいた。 | `.socium`配下のdirectory/fileリンク、非通常ファイル、hardlink、他ユーザー所有を拒否。Unixではdirectory 0700・保存ファイル0600。各操作時の検査、読込サイズ制限、親差替え・リンク・旧権限の回帰試験。 |
| Git補助プログラム | status/diffでも設定済みclean/process filterが起動し得る。読取りツールからコマンド承認を経ずに実行できた。 | 外部filter設定があるrepoでは専用Gitツールを拒否し、明示承認するhost commandを案内。外部diff/textconv・fsmonitor・署名helperを無効化し、submodule内部走査とlazy fetchを抑止。fixtureのmarkerファイルが作られないことを検証。 |
| Git metadataの別名 | `.git`自体がworkspace内の別directoryへのsymlinkの場合、canonical pathだけの名前判定ではmetadataを編集できた。 | `.git`の実体も保護対象とし、直接path・別名・新規hook pathが拒否されることを検証。 |
| HTTPリダイレクト・エラー本文 | APIキー付きclientが既定のredirect policyを使用。非成功レスポンス本文を無制限に読み、そのまま表示していた。 | redirectを禁止し、APIキーheaderをsensitive指定。HTTPエラーはstatusだけを返し、本文を読込・保存しない。ローカル307応答で別endpointへの転送がないことを検証。 |
| CI権限 | Checkout credentialの残留、PR実行へのcache書込token受渡し、workflow lintの権限指定不足。 | credential永続化を停止。Cachix書込tokenはmainへのpush時だけ渡す。contents:readを指定し、秘密情報・依存監査のCIを追加。 |

Git filterの挙動は[Git公式仕様](https://git-scm.com/docs/gitattributes)、HTTPの設定は[reqwestのredirect仕様](https://docs.rs/reqwest/0.13.5/reqwest/redirect/)を参照。APIの固定endpointが実際に不正redirectを返したとする報告ではなく、防御上の修正。

## 検証

- Gitleaks 8.30.0: ローカルの全reachable refs、173コミットを検査し検出0。秘密値はredact指定。最新treeも公開対象124ファイルを別ディレクトリへコピーして検査し、検出0。
- cargo-audit 0.22.1: RustSec DB `901cf64f2d04dc7f9f26bbbd50a3010378de034d`で脆弱性0・警告0。
- Nix開発環境でfmt、Clippy（warnings deny）、全131テスト（core117、TUI14）が成功。実API・課金なし。
- actionlintでGitHub Actionsの構文を検証。Nix package buildと最新commitの最終結果は、この変更のPR checksで確認する。

```sh
nix develop --command cargo fmt --all --check
nix develop --command cargo clippy --locked --all-targets --all-features -- -D warnings
nix develop --command cargo test --locked --all-targets --all-features
nix shell --inputs-from . nixpkgs#gitleaks nixpkgs#cargo-audit --command \
  bash -c 'gitleaks git . --log-opts="--all" --redact=100 && cargo audit --deny warnings'
nix build .#default
```

## 残る境界

これは安全性の証明ではなく、上記範囲の検証結果。秘密情報検出は既知pattern中心で、GitHub上のIssue・添付物・Actionsログ・他サービスはスキャン対象外。

OS sandboxは未実装（#55）。承認済みコマンドはhost権限を持ち、同じOSユーザーによる並行path差替えを防ぐrace-freeな隔離ではない。workspace本文からのprompt injectionもモデル任せで完全に防止できない。信頼したworkspaceで使い、コマンド・diffを確認する。

状態・出力は平文。実モデル通信、macOS、Windowsは今回検証していない。move/deleteのundoにおけるfile mode復元は#93で未完了。リポジトリを実際に公開する前には、GitHubの公開範囲・private vulnerability reporting・required checksも所有者の設定として確認する。

GitHub設定の読取りではリポジトリはprivate、mainのbranch protectionは無効。private vulnerability reportingの照会は404で、利用可否は確認できなかった。公開準備のコード修正に伴って可視性や保護ルールは変更していない。公開時にrequired checksと非公開の報告窓口を設定する。
