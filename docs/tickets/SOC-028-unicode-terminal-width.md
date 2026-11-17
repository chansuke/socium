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

2026-11-17 完了（branch `fix/grapheme-input`）。

- 表示側の幅計算と切詰めは既に `crates/tui/src/view/cells.rs` が grapheme と cell 幅で行っていた。残っていたのは入力欄の編集で、左右移動と Backspace が char 単位だったため、結合文字（e + U+0301）、ZWJ 絵文字、国旗の途中で止まり、一部だけを消していた。
- `App` のカーソル移動・削除・上下移動を grapheme 境界に揃えた。`character_index` は char 数のまま、常に境界に置く。
- 試験: `editing_moves_and_deletes_whole_graphemes`、`moving_up_or_down_lands_on_a_grapheme_boundary`（修正前に失敗を確認）。既存の全角カーソル位置・小画面の試験も通過。
- 保証範囲: 幅は `unicode-width` による。ZWJ 列や国旗を複数の字形で描く端末では、表示が計算より広くなる（`cells.rs` に明記）。実端末ごとの確認はしていない。

[一覧へ戻る](README.md)

