# SOC-035: 実行中の方針修正とfollow-up入力を安全に受け付ける

| 項目 | 値 |
|---|---|
| GitHub Issue | [#87](https://github.com/chansuke/socium/issues/87) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`enhancement`、`area:tui`、`area:runtime`、`status:blocked` |
| GitHub前提 | [SOC-005 / #57](https://github.com/chansuke/socium/issues/57)、[SOC-011 / #63](https://github.com/chansuke/socium/issues/63)、[SOC-017 / #69](https://github.com/chansuke/socium/issues/69) |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F10／pi loop比較 |
| 前提チケット | [SOC-005](SOC-005-durable-turn-events.md)、[SOC-011](SOC-011-cancellation-and-worker-recovery.md)、[SOC-017](SOC-017-per-request-context-budget.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

進行中は通常入力を拒否するため、ユーザーが追加要件や修正を伝えても次の安全な境界で反映する経路がない。

**対応範囲**

- steeringと次turn用follow-upのキュー・順序・消費境界を定義する。
- 受付済み入力を永続記録し、tool実行中は安全なstep境界まで待つ。
- cancel/approval等の制御入力と通常入力を分離してUIへ状態を表示する。

**受け入れ条件**

- [ ] 進行中の入力が重複・欠落せず、どのrequestから反映されたか確認できる。
- [ ] 実行中toolの副作用を途中から別要求へ読み替えず、必要なら明示的cancelを使う。
- [ ] restart後も未消費follow-upを識別し、勝手に二重実行しない。
- [ ] 追加入力を含むcontextがSOC-017の予算検査を通る。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 複数追加入力、compact中入力、cancelと入力の競合、restartを試験する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/tui/src/app.rs](../../crates/tui/src/app.rs)
- [crates/tui/src/agent_task.rs](../../crates/tui/src/agent_task.rs)
- [crates/core/src/agent/turn.rs](../../crates/core/src/agent/turn.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)

**対象外・境界**

subagentsや自律的な無期限継続は含めない。

**完了記録**

2026-11-17 完了（branch `feat/steering`）。

- 実行中の Enter は `TaskControl::steer` のキューに入る（コマンドは従来どおり終了を待つ。cancel・承認のキーは別経路のまま）。
- main agent の turn だけが、round の tool がすべて終わり結果を保存した後、次の request の前にキューを取り出し、tool 結果の user メッセージに印付きの text として加えて保存する。実行中の tool の副作用を別の要求に読み替えない。追加分は次の round の予算検査に含まれる。どの round で読んだかは Activity の `UserSteered` に出る。サブエージェントは取り出さない。
- turn の終了時に読まれずに残った入力は、正常終了なら次の turn として送り、中断・失敗なら入力欄に戻す（勝手に実行しない）。
- 再起動: 読まれた入力は会話に保存され、transcript には利用者の言葉として出る。読まれる前の入力はプロセス内だけにあり、入力履歴（↑）に残るが自動では実行しない。
- 試験: `words_added_mid_turn_reach_the_next_request_after_the_tools_finish`（tool 実行前の request には入らない、一度だけ取り出す、保存と再起動後の表示）、`words_added_to_a_turn_that_ends_without_tools_are_left_for_the_next`、TUI の `enter_during_a_request_hands_the_words_to_it_and_commands_wait`、`unread_words_follow_a_finished_turn_and_return_after_a_cancelled_one`。
- 未検証: 実端末での操作感。

[一覧へ戻る](README.md)

