# SOC-024: 検索の引数・結果形式・fallback・資源上限を統一する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#76](https://github.com/chansuke/socium/issues/76) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`bug`、`area:tools`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | [SOC-025 / #77](https://github.com/chansuke/socium/issues/77) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 検索・検索の境界 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

patternをoptionと分離せず、colon区切りをparseする。全結果回収後に件数制限し、rgなしではregexが部分文字列検索に変わる。

**対応範囲**

- -e patternと--等で引数境界を固定し、rg JSON等の構造化出力へ移行する。
- regex/fixed/smart-case/glob/ignore/symlinkの契約を定義し、fallbackの差を無言で許容しない。
- timeout・結果件数・bytes・単一行長を取得中から制限し、途中結果と打切り理由を返す。

**受け入れ条件**

- [x] 先頭hyphenのpatternとcolonを含むpathでも誤解釈せず検索できる。
- [x] rgあり/なしで同じ対応機能の結果が一致し、非対応regex等は明示エラーとなる。
- [x] 巨大repo・長大行・大量一致でも時間とメモリが設定範囲に収まる。
- [x] 既存のpath:line参照、case、glob、ignore規約が検証可能な形で維持される。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 特殊filename、空一致、invalid regex、rg不在、強制timeout、binary fixtureを試験する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/ripgrep.rs](../../crates/core/src/tools/ripgrep.rs)
- [crates/core/src/tools/search_text.rs](../../crates/core/src/tools/search_text.rs)
- [crates/core/src/tools/list_files.rs](../../crates/core/src/tools/list_files.rs)

**対象外・境界**

symbolの言語拡張はSOC-025。

**完了記録**

rg -e/--とJSONでhyphen pattern・colon filenameを扱い、regex/ignore等の異なる無言fallbackを廃止。rgがなければ明示エラー。検索5秒・256 KiB、結果件数、listingの時間/走査量/深さ/件数を制限し、切詰めを表示。Nix package/devShellへrgを追加。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

