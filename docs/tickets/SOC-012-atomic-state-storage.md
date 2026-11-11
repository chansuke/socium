# SOC-012: 状態保存を原子的にしwriter競合と破損から回復する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#64](https://github.com/chansuke/socium/issues/64) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`bug`、`area:state`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | [SOC-005 / #57](https://github.com/chansuke/socium/issues/57)、[SOC-009 / #61](https://github.com/chansuke/socium/issues/61)、[SOC-013 / #65](https://github.com/chansuke/socium/issues/65)、[SOC-026 / #78](https://github.com/chansuke/socium/issues/78) |
| 状態 | 実装・検証済み（マージ状態はGitHubを参照） |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F11（物理保存） |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

fs::writeによる全体上書きとconversation/summaryの別保存により、中断や同時起動で破損・世代不一致が起き得る。読み込み失敗時に空会話へ進む。

**対応範囲**

- temp-file＋rename等でsnapshotを保存し、conversation/summaryの同一世代を識別する。
- 単一writerの所有権、schema version、flush方針、破損時の原本保全と復旧手順を定義する。
- SOC-005の逐次ログを収容する保存APIを設け、append tail切断と未知versionへの挙動を明確にする。

**受け入れ条件**

- [x] 書込み各段階の中断後、旧または新の一貫した状態を復元でき、summaryだけ別世代にならない。
- [x] 同じsessionへの二重writerを拒否し、別processの結果を黙って上書きしない。
- [x] 壊れたファイルや未知versionを空会話として上書きせず、原本と診断情報を残す。
- [x] append形式では未完tailを検出・隔離でき、正常なprefixを再読込できる。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- temp作成・write・flush・rename・pair更新・lock競合への故障注入を行う。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/agent/state.rs](../../crates/core/src/agent/state.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/core/src/tools/edit_history.rs](../../crates/core/src/tools/edit_history.rs)

**対象外・境界**

modelに投影する履歴範囲はSOC-013。named session UIはSOC-026。P2はレビュー由来の重大度であり、SOC-005の前提として早期着手する。

**完了記録**

schema付きの単一snapshotにconversation/summary/eventsを保存。create-new temp→write→sync→rename→directory sync、単一writer OS lock、64 MiB上限、壊れた/未知versionの原本保全を実装。各保存境界への故障注入と別processのlock競合を検証。append JSONLは採用していないため未完tail条件は非該当。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。

[一覧へ戻る](README.md)

