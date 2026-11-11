# SOC-023: tool結果を構造化し大容量出力をartifact化する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#75](https://github.com/chansuke/socium/issues/75) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`enhancement`、`area:tools`、`area:runtime`、`status:blocked` |
| GitHub前提 | [SOC-004 / #56](https://github.com/chansuke/socium/issues/56) |
| GitHub後続 | [SOC-017 / #69](https://github.com/chansuke/socium/issues/69)、[SOC-027 / #79](https://github.com/chansuke/socium/issues/79) |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 ツールの出力・turn内増加 |
| 前提チケット | [SOC-004](SOC-004-process-lifecycle.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

tool結果の多くが文字列だけで、exit/truncationを後段が再解釈する。大きな出力をcontextとメモリへ持ち込みやすい。

**対応範囲**

- textとexit/error/truncation/bytes/path等のmetadataを分離する。
- 大容量原文をartifactとして保持し、範囲読取・保持期限・アクセス権を定義する。
- command以外のgit/search/read結果にも共通の出力予算を適用する。

**受け入れ条件**

- [ ] UIとagentが終了・切詰めを自然言語parseせず判定できる。
- [ ] 上限超過時に原文参照と省略範囲が分かり、必要箇所をboundedに再取得できる。
- [ ] 失敗やstderr末尾を先頭だけの切詰めで見失わない。
- [ ] artifact参照もworkspace/policy境界を守り、期限切れや欠落を明示する。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- 大容量git diff/search/command出力、UTF-8境界、artifact期限切れを試験する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/core/src/tools/mod.rs](../../crates/core/src/tools/mod.rs)
- [crates/core/src/tools/run_command.rs](../../crates/core/src/tools/run_command.rs)
- [crates/core/src/tools/git.rs](../../crates/core/src/tools/git.rs)
- [crates/core/src/tools/read_file.rs](../../crates/core/src/tools/read_file.rs)
- [crates/core/src/agent/types.rs](../../crates/core/src/agent/types.rs)

**対象外・境界**

永続storageサービスや任意ファイルの公開URL化は含めない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

