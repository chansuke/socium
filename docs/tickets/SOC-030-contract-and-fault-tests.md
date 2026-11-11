# SOC-030: wire契約と故障注入の共通テスト基盤を整える

| 項目 | 値 |
|---|---|
| GitHub Issue | [#82](https://github.com/chansuke/socium/issues/82) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`type:testing`、`area:quality`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | [SOC-032 / #84](https://github.com/chansuke/socium/issues/84) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 品質基盤／評価方法 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

既存108テストは通るが、mockが不正requestを受理し、権限・pipe・保存・途中失敗の契約を十分検証しない。

**対応範囲**

- ScriptedProviderにrequest validatorと故障タイミングを注入できる支援を追加する。
- HTTP/SSE再生、fake clock、state write failure、worker/process故障の再利用fixtureを設ける。
- 修正チケットごとに観察用テストの期待を安全な振る舞いへ反転し、通常CIへ移す。

**受け入れ条件**

- [ ] 不正role/tool pair/引数をfixtureが黙って受理しない。
- [ ] 元の15観察条件に対応する修正チケットと回帰試験の所在を追跡できる。
- [ ] 未修正の欠陥をassertする観察テストを、安全性の合格試験としてCIに登録しない。
- [ ] 故障試験が実credential/networkに依存せず、watchdogでハングを抑止する。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 他の修正チケットのテスト移行表と、基盤自体の正常/異常fixtureを検証する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/test_support.rs](../../crates/core/src/test_support.rs)
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs)
- [crates/core/src/providers/anthropic/stream.rs](../../crates/core/src/providers/anthropic/stream.rs)
- [.github/workflows/ci.yaml](../../.github/workflows/ci.yaml)
- [docs/review-evidence/2026-11-11/review_repros.rs](../../docs/review-evidence/2026-11-11/review_repros.rs)
- [docs/review-evidence/2026-11-11/stream_repros.rs](../../docs/review-evidence/2026-11-11/stream_repros.rs)

**対象外・境界**

各不具合修正の回帰テストをこのチケット完了まで延期しない。実モデル性能評価はSOC-032。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

