# SOC-028: 日本語・結合文字・絵文字の入力と表示幅を修正する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#80](https://github.com/chansuke/socium/issues/80) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`bug`、`area:tui`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 文字幅 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

chars数をterminal幅として扱う箇所があり、全角・結合文字・絵文字でcursor位置や折返しがずれ得る。

**対応範囲**

- byte index、grapheme編集単位、terminal cell幅を分離する。
- 入力window・cursor移動・削除・折返し・truncationで共通の幅計算を使う。
- 複合絵文字等のterminal差と保証範囲を明示する。

**受け入れ条件**

- [ ] 日本語を含む入力でcursor表示と実挿入位置が一致する。
- [ ] 結合文字や対応するgraphemeを途中で壊す削除・切詰めを行わない。
- [ ] CJKとASCII混在の会話・code・pathを指定cell幅に収める。
- [ ] 空文字・幅1・resize・長い非ASCII入力でpanicしない。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- CJK、濁点結合、emoji/ZWJ、全角記号をfixture化してrenderと編集を確認する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/tui/src/app.rs](../../crates/tui/src/app.rs)
- [crates/tui/src/view/text_layout.rs](../../crates/tui/src/view/text_layout.rs)
- [crates/tui/src/transcript.rs](../../crates/tui/src/transcript.rs)

**対象外・境界**

全terminal emulatorの異なるemoji描画を同一にすることは保証しない。

**完了記録**

未実施。実装PR/commit、検証結果、残課題を記入してから状態を更新する。

[一覧へ戻る](README.md)

