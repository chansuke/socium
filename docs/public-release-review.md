# 公開準備・セキュリティ確認

この文書はPR #96時点の公開準備記録。後続のOS隔離（#55）の現在の仕様は[Linux sandbox guide](linux-sandbox.md)を参照。

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

PR #96時点ではOS sandboxは未実装（後続の#55で対応）。当時の承認済みコマンドはhost権限を持ち、同じOSユーザーによる並行path差替えを防ぐrace-freeな隔離ではない。workspace本文からのprompt injectionもモデル任せで完全に防止できない。信頼したworkspaceで使い、コマンド・diffを確認する。

状態・出力は平文。実モデル通信、macOS、Windowsは今回検証していない。move/deleteのundoにおけるfile mode復元は#93で未完了。リポジトリを実際に公開する前には、GitHubの公開範囲・private vulnerability reporting・required checksも所有者の設定として確認する。

GitHub設定の読取りではリポジトリはprivate、mainのbranch protectionは無効。private vulnerability reportingの照会は404で、利用可否は確認できなかった。公開準備のコード修正に伴って可視性や保護ルールは変更していない。公開時にrequired checksと非公開の報告窓口を設定する。

## 2026-11-15 の再確認

基準: PR #121 (`fix/review-2026-11-14`) と、その上の `docs/open-source-prep`。公開設定の変更、GitHub への書き込み、実モデル API への送信はしていない。

| 項目 | 結果 |
|---|---|
| 秘密情報 (履歴) | Gitleaks 8.30.0 で全 ref の 349 コミットを検査し、検出 0。 |
| 秘密情報 (Issue・PR) | 公開すると見えるようになる Issue 47 件 (コメント 1 件) と PR 74 件の本文・コメントを書き出して Gitleaks で検査し、検出 0。 |
| 依存の脆弱性 | cargo-audit (RustSec 1,509 件) で脆弱性 0・警告 0。 |
| 依存のライセンス | 345 crate はすべて MIT・Apache-2.0・BSD・ISC・Zlib・Unicode-3.0 など許容的なもので、MIT での公開と両立する。`r-efi` の LGPL-2.1-or-later は MIT・Apache-2.0 との選択式。バイナリを配布するときは各ライセンス表示を同梱する。 |
| ローカルの情報 | 2026-11-11 のレビューと証拠に残っていた絶対パスを相対リンクと `<repo>` に置き換えた。追跡中のファイルに `/home/...` は無い。 |
| ビルド | Nix の package build (全テスト込み) が成功。Nix の開発シェルを使わず、Cargo・rustc 1.92・gcc・CMake だけの環境でも release build が成功した。 |
| crates.io の名前 | `socium`、`socium-core`、`socium-tui` はどれも未使用 (2026-11-15 に API で確認)。 |
| CI | action は SHA で固定。PR には Cachix の書き込み token を渡さず、`pull_request_target` を使っていないので、fork からの PR でも秘密は渡らない。 |

### 公開すると見えるようになるもの

- コミットの作者・コミッターの個人のメールアドレス 2 つ。
- main の 46 コミットのメッセージと、PR 14 件の本文にある Claude Code のセッション URL。開くにはログインが要るが、セッション ID は見える。
- Issue 47 件、`docs/tickets/`、レビューの記録 (日本語)。

履歴を書き換えて force push しても、PR が参照する古いコミット (`refs/pull/*`) は読み取り専用で残り、PR のページから辿れる。消すには GitHub Support への依頼が要る ([Removing sensitive data from a repository](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository))。そこで、今のリポジトリは `socium-archive` に改名して private のまま残し、メールアドレスを GitHub の noreply に置き換えセッション URL の行を除いた履歴を、新しい `chansuke/socium` に置いて公開する。Issue は移管する ([private から public へは移管できない](https://docs.github.com/en/issues/tracking-your-work-with-issues/administering-issues/transferring-an-issue-to-another-repository) ので、新しいリポジトリが private のうちに移す)。PR とその議論は archive に残り、文書中の PR 番号は archive のものを指す。

### 公開した後でないと設定できないもの

無料プランの private repository では、branch protection と private vulnerability reporting を有効にできない (API が 403 と 404 を返す)。公開した直後に次を設定する。

- private vulnerability reporting を有効にする (`SECURITY.md` と issue form の窓口がこれを指す)。
- main に ruleset を置き、PR を必須にし、CI の `Format, Lint and Test`、Security の `Secrets and dependencies`、`Validate PR title` を必須の check にし、force push と削除を禁止する。

### 所有者が決めること

2026-11-15 に決めたこと: コマンド名は `socium` (パッケージ名は `socium-tui` のまま)、crates.io には公開しない (`publish = false`)、最初のタグを打つ、作者のメールアドレスとセッション URL を消すために履歴を書き換えて新しいリポジトリで公開する、古い PR は閉じる (#120 を閉じた。#116 は新しい機能なので archive に残す)。

残り:

- リポジトリの説明 (今は "A tiny coding agent for personal use") と topics。
- Code of Conduct を置くか、Discussions を開くか。

## 2026-11-19 の再確認

基準: main `d4d785d` (書き換え前)。Codex の `gpt-5.6-sol` に公開してよい品質かをレビューさせ、指摘を裏取りしてからチケットごとに直した。各修正も Sol にレビューさせ、指摘を直すか、直さない理由をチケットに書いた。

| チケット | PR | 内容 |
|---|---|---|
| [SOC-043](tickets/SOC-043-protect-cargo-manifests.md) | #101 | `Cargo.toml` と `Cargo.lock` の編集に承認を求める |
| [SOC-044](tickets/SOC-044-sensitive-credential-files.md) | #97 | `auth.json` などの認証情報ファイルを読むときに承認を求める |
| [SOC-045](tickets/SOC-045-subagent-concurrency-cap.md) | #96 | 同時に動くサブエージェントを 4 つまでにする |
| [SOC-046](tickets/SOC-046-responses-unfinished-calls.md) | #103 | Responses API で終わっていない tool 呼び出しを実行しない |
| [SOC-047](tickets/SOC-047-version-flag.md) | #99 | `socium --version` |
| [SOC-048](tickets/SOC-048-approval-wording.md) | #102 | README の承認の説明を実装に合わせる |
| [SOC-049](tickets/SOC-049-commit-metadata-guard.md) | #104 | セッション URL と個人のアドレスをコミットに入れない |
| [SOC-050](tickets/SOC-050-third-party-notices.md) | #105 | パッケージに依存 crate のライセンス表示を入れる |

直さなかったもの: web search の承認と回数の上限 (ユーザーが有効にする機能で、送り先は設定に書いてある)、ワークスペース内の proc-macro crate のソースの編集 (承認を求めるかは決めていない。[脅威モデル](threat-model.md)の「守らないもの」に書いた)。

### 履歴

2026-11-15 の書き換えの翌日に、セッション URL 入りのコミット 16 件と、作者が個人のアドレスのコミット 2 件が main に入っていた。2026-11-19 に main と開いている 7 ブランチ (`mcp-*`、`sandbox-env-setenv`) を書き換えた。

- メッセージからセッション URL の行を消し、個人のアドレスを noreply にした。対象は 8f1bb83 以降のコミットだけで、それより前のコミットとタグ `v0.1.0` のハッシュは変わらない。
- 各ブランチの最後のコミットの tree とコミット数は書き換える前と同じ。開いている PR のコミット数も変わらない。
- PR #50〜#55 の本文からセッション URL の行を消した。
- 同じものがまた入らないよう、commit-msg フックと Commit metadata ワークフローを足した (SOC-049)。

### 公開する前に残っていること

- **GitHub Support への依頼:** 古いコミットは、PR #48、#50〜#89、#95〜#105 の `refs/pull/*` から辿れる。読み取り専用で、消すには Support に頼む ([Removing sensitive data from a repository](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository))。PR #50〜#55 の本文の編集履歴にも古い本文が残る。編集履歴の版は所有者が PR のページから消せる。
- **CI:** 課金の問題で Actions のジョブが始まらない。公開する commit で CI、Security、Commit metadata を通す。
- **公開したらすぐに設定すること:** rebase merge だけを許す (merge と squash のコミットは PR のタイトルと本文から作られ、検査を通らない)。main に ruleset を置き、PR を必須にし、`Format, Lint and Test`、`Secrets and dependencies`、`Validate PR title`、`Check commit metadata` を必須の check にし、force push と削除を禁止する。private vulnerability reporting を有効にする。
