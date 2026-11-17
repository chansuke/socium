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

2026-11-17 完了（branch `test/wire-contract-fixtures`）。

- `ScriptedProvider` が受け取る request を毎回 `test_support::check_request` で検査し、どの provider も受け付けない request なら test を失敗させる。user/assistant 以外の role、空の message・text block、user で始まり user で終わらない履歴、次の message に result のない tool call、call のない result、object でない tool 入力、名前の重複する・object の schema を持たない tool 定義、0 の max_tokens を拒否する。導入時点の既存 491 件はすべて通過（不正な request を送っていた試験はなかった）。検査そのものは `request_checks` の試験で固定した。
- 故障の注入: provider の失敗は `ScriptedProvider` の応答列、HTTP・SSE の失敗と遅延は `test_server::{serve_once, serve_paced}`、状態保存の失敗は `atomic_write_with_checkpoint` と reliability_tests の checkpoint 失敗で既に注入できる。
- watchdog: loopback の試験 server は接続と読み取りを 10 秒で打ち切り、理由付きで失敗する。実 credential・外部 network には依存しない。
- 15 件の観察条件と修正チケット・回帰試験の対応を [regression-map.md](../review-evidence/2026-11-11/regression-map.md) に記録した。観察試験そのものは一時コピーで実行したもので build に含まれず、CI に安全性の合格試験として登録されていない。
- 範囲外: fake clock。時間に依存する試験は短い実時間（再試行の間隔など）で足りている。

[一覧へ戻る](README.md)

