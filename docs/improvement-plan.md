# 旧改善計画の整理

2026-11-11の公開準備で、旧設計案を現コードとSOCチケットへ照合した。
以前のコード断片・旧ファイル構成・未検証の完了判断は削除し、旧task IDは以下に残す。
旧文書の全文はGit履歴で確認できる。今後の計画と受け入れ条件は
[改善チケット](tickets/README.md)、実行状態は[GitHub #52](https://github.com/chansuke/socium/issues/52)を正とする。

`PLAN-*`はこの文書の旧番号、`TODO-*`は[todo.md](todo.md)の旧番号であり、別系列。
「部分実装」は機能の存在を示すだけで、後続チケットの完了を意味しない。

| 旧ID | 項目 | 現状と引継ぎ |
|---|---|---|
| PLAN-01 | ツールモジュール分割 | 完了。`crates/core/src/tools/`に分離済み。旧runtime.rsの分割作業は不要。 |
| PLAN-02 | 型付きツール入力 | 部分実装。各ツールに入力解析あり。共通の出力・契約整備はSOC-014 / SOC-023。 |
| PLAN-03 | リトライとエラー分類 | 未完了。SOC-019。中断・逐次保存は実装済みだが自動retryは未実装。 |
| PLAN-04 | システムプロンプト | 部分実装。`constants.rs`に最小規約あり。project instructionsはSOC-022。 |
| PLAN-05 | 中断 | 完了。SOC-011 / SOC-038。HTTP・承認・コマンドの中断と結果保存を検証。 |
| PLAN-06 | スラッシュコマンド | 基本操作は実装済み。`crates/tui/src/app.rs`。複数sessionはSOC-026。 |
| PLAN-07 | コスト追跡 | 未完了。SOC-020。最新requestのtoken計測と全attemptの費用集計は別。 |
| PLAN-08 | 設定ファイル | 未着手。SOC-015。現在は環境変数。架空の自動承認クラスは公開仕様にしない。 |
| PLAN-09 | Extended Thinking | 部分実装。環境変数・stream表示あり。model capability検証はSOC-015、provider契約はSOC-014。 |
| PLAN-10 | SOCIUM.mdメモリ | 未着手。scope付きAGENTS.mdのSOC-022へ置換。 |
| PLAN-11 | MCPクライアント | 保留。SOC-036の需要・費用評価後に採否判断。 |
| PLAN-12 | セッション管理 | 部分実装。同一workspaceの再開はSOC-005 / SOC-012で実装。複数sessionはSOC-026。 |
| PLAN-13 | LLM要約 | 部分実装。`agent/compaction.rs`に実装。品質・予算はSOC-017 / SOC-018。 |
| PLAN-14 | 構造化diff表示 | 部分実装。完全なbefore/after previewあり。構造化tool結果・表示はSOC-023 / SOC-027。 |
| PLAN-15 | 並行ツール実行 | 保留。SOC-036で採否判断。現在の逐次実行・順序保証を優先。 |

旧KAIROS/MCP/常駐処理等の拡張案を公開ロードマップの約束にはしない。
現在の優先順位は日常作業の信頼性、設定とproviderの整備、評価可能な拡張。
[運用と保証範囲](daily-reliability.md)および[検証記録](daily-reliability-validation.md)を参照。
