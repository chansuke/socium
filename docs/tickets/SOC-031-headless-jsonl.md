# SOC-031: 自動検証できるheadless JSONL実行経路を追加する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#83](https://github.com/chansuke/socium/issues/83) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`enhancement`、`area:delivery`、`area:runtime`、`status:blocked` |
| GitHub前提 | [SOC-001 / #53](https://github.com/chansuke/socium/issues/53)、[SOC-005 / #57](https://github.com/chansuke/socium/issues/57)、[SOC-007 / #59](https://github.com/chansuke/socium/issues/59)、[SOC-011 / #63](https://github.com/chansuke/socium/issues/63)、[SOC-015 / #67](https://github.com/chansuke/socium/issues/67) |
| GitHub後続 | [SOC-032 / #84](https://github.com/chansuke/socium/issues/84) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — pi接続形態／実装順7／評価方法 |
| 前提チケット | [SOC-001](SOC-001-trusted-approval.md)、[SOC-005](SOC-005-durable-turn-events.md)、[SOC-007](SOC-007-stream-completion.md)、[SOC-011](SOC-011-cancellation-and-worker-recovery.md)、[SOC-015](SOC-015-validated-model-config.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

自動実行・故障評価・harness比較のためのheadless入出力契約がなく、TUI操作に依存しやすい。

**対応範囲**

- coreを再利用するheadless入口とversion付きJSONL event/control schemaを定義する。
- submit/cancel/approval responseとprogress/result/failureを相関ID付きで扱う。
- 終了コード、stderr診断、stdin閉鎖、backpressure、timeoutを明確にする。

**受け入れ条件**

- [ ] TUIなしで一つのtaskを実行し、tool・usage・結果を機械的に取得できる。
- [ ] stdoutにはschemaに従ったJSONLだけを出し、診断はstderrへ分離する。
- [ ] approval answerer不在を暗黙許可にせず、cancel/入力切断で安全に収束する。
- [ ] 同じcore fixtureがTUI経路とheadless経路で同じ実行事実を返す。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- subprocess経由のstdin/stdout契約、部分行、malformed control、receiver切断を試験する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/tui/src/main.rs](../../crates/tui/src/main.rs)
- [Cargo.toml](../../Cargo.toml)

**対象外・境界**

公開network server、Web UI、全面RPC framework導入は含めない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

