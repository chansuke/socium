# SOC-019: API障害を分類し安全な範囲でretryする

| 項目 | 値 |
|---|---|
| GitHub Issue | [#71](https://github.com/chansuke/socium/issues/71) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`enhancement`、`area:provider`、`area:runtime`、`status:blocked` |
| GitHub前提 | [SOC-005 / #57](https://github.com/chansuke/socium/issues/57)、[SOC-007 / #59](https://github.com/chansuke/socium/issues/59)、[SOC-011 / #63](https://github.com/chansuke/socium/issues/63) |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 API障害 |
| 前提チケット | [SOC-005](SOC-005-durable-turn-events.md)、[SOC-007](SOC-007-stream-completion.md)、[SOC-011](SOC-011-cancellation-and-worker-recovery.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

一時的HTTP障害の型付き分類やbackoffがなく、stream途中失敗の再試行を安全に表現する契約もない。

**対応範囲**

- 429・一時5xx・接続失敗・認証/入力エラー・途中stream失敗を構造化する。
- Retry-After、上限回数/総時間、jitter付きbackoff、cancelを実装する。
- attempt IDで部分出力を区別し、provider requestの再試行で既実行toolを再実行しない。

**受け入れ条件**

- [ ] 一時エラーだけが規定上限内でretryされ、401/403/不正入力は即時診断される。
- [ ] backoff中のcancelが速やかに反映され、全体deadlineを超えて継続しない。
- [ ] 部分streamの再試行がUI上の二重確定やtool副作用の重複を生まない。
- [ ] 各attemptの失敗と最終結果が追跡でき、credentialをログへ出さない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- fake clockとHTTP fixtureでRetry-After、連続失敗、途中EOF、cancelを試験する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/providers/error.rs](../../crates/core/src/providers/error.rs)
- [crates/core/src/providers/anthropic/mod.rs](../../crates/core/src/providers/anthropic/mod.rs)
- [crates/core/src/providers/mod.rs](../../crates/core/src/providers/mod.rs)
- [crates/core/src/agent/turn.rs](../../crates/core/src/agent/turn.rs)
- [crates/tui/src/progress.rs](../../crates/tui/src/progress.rs)

**対象外・境界**

副作用のあるturn全体を最初から自動再実行する実装にはしない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

