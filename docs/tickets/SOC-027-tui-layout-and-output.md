# SOC-027: 小画面の会話領域とコマンド失敗表示を改善する

| 項目 | 値 |
|---|---|
| GitHub Issue | [#79](https://github.com/chansuke/socium/issues/79) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`bug`、`area:tui`、`status:blocked` |
| GitHub前提 | [SOC-015 / #67](https://github.com/chansuke/socium/issues/67)、[SOC-023 / #75](https://github.com/chansuke/socium/issues/75) |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 UI |
| 前提チケット | [SOC-015](SOC-015-validated-model-config.md)、[SOC-023](SOC-023-structured-tool-output.md) |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

固定10行headerと8行補助paneが小画面を圧迫し、stdout先頭の表示でstderrや末尾エラーが隠れやすい。

**対応範囲**

- 画面サイズに応じてheader/activity/outputの高さ・折畳みを調整する。
- 失敗理由、stderr/末尾、truncation、artifactへの導線を優先する。
- 実設定・実状態からmodelと進捗を表示し、内容閲覧のscroll操作を整える。

**受け入れ条件**

- [ ] 80×24程度のterminalでも入力と会話、失敗状態を確認できる。
- [ ] 大量stdoutの後に出たstderrエラーへ明示的にアクセスできる。
- [ ] resize時にpanicせず、狭い画面で重要操作が不可視にならない。
- [ ] 表示model・実行中/失敗/中断状態がcoreのイベントと一致する。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- Ratatui TestBackend等で複数サイズと成功/失敗/切詰め状態を確認する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [crates/tui/src/view/mod.rs](../../crates/tui/src/view/mod.rs)
- [crates/tui/src/view/text_layout.rs](../../crates/tui/src/view/text_layout.rs)
- [crates/tui/src/transcript.rs](../../crates/tui/src/transcript.rs)
- [crates/tui/src/progress.rs](../../crates/tui/src/progress.rs)

**対象外・境界**

テーマ刷新やWeb UIの実装は含めない。

**完了記録**

2026-11-17 一部（branch `fix/command-output-stderr`）。

- Activity で展開したコマンド出力が stdout を全部並べた後に stderr を置き、行数で切っていたため、長い stdout の後の stderr が見えなかった。失敗時は stderr を先に出し、二つの stream が行数を分け合い、それぞれ末尾を残して「… N lines above」と示すようにした。試験 `a_failure_shows_stderr_and_the_end_of_a_long_stdout`（修正前に失敗を確認）。
- 残り: 80×24 での header・Activity の高さ調整の再評価、表示 model と状態が core のイベントと一致することの確認。

[一覧へ戻る](README.md)

