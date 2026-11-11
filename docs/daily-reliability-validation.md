# 日常作業の信頼性: 実装・検証記録

検証日: 2026-11-11。実API呼出し・課金は行っていない。

```sh
nix develop --command cargo fmt --all --check
nix develop --command cargo clippy --all-targets --all-features -- -D warnings
nix develop --command cargo test --all-targets --all-features
```

全125テスト成功（core 111、TUI 14）。CIでは加えてNix package buildを実行する。
実行結果・マージ状態は、この変更を含むGitHub PRのchecksを参照。

| 対象チケット | 実装と検証内容 |
|---|---|
| [SOC-001 / #53](https://github.com/chansuke/socium/issues/53) | モデルの自己申告フィールドを除去し、workspace・tool call・引数に紐付く120秒・一回限りの承認をTUIへ接続。偽造、古いID、重複応答、拒否、期限切れ、中断、応答経路なしを拒否。許可後の実行境界も副作用より前に保存。 |
| [SOC-002 / #54](https://github.com/chansuke/socium/issues/54) | 未作成pathもcomponent単位で正規化し、symlinkを解決してworkspace境界を検証。missing/../../、dangling symlink、親symlink、絶対pathを副作用前に拒否。並行symlink差替えに対するOS隔離とは区別して文書化。 |
| [SOC-004 / #56](https://github.com/chansuke/socium/issues/56) | stdout/stderrをnonblockingに並行drain。256 KiB/streamのメモリ上限、8 MiB/streamのartifact、進捗通知、Unix process groupの終了と200 msのcleanup上限を実装。両pipe同時2 MiB、親終了後の継承pipe、signal、cancelを検証。 |
| [SOC-005 / #57](https://github.com/chansuke/socium/issues/57) | ユーザー入力、assistant intent、承認済み実行開始、各tool結果、turn終了/失敗を同一snapshotへ逐次保存。turn offset・round・attempt・tool ID・event sequenceを記録。第二provider失敗、round上限、cancel、実行前/後の保存失敗と再開時Unknownを検証。 |
| [SOC-006 / #58](https://github.com/chansuke/socium/issues/58) | 予算に収まる25件はそのまま保持。通常/緊急/明示compactionでtool_result境界を共通化し、summaryをuser dataとして投影。tool ID対応を送信前に検証し、予算超過は明示エラー。 |
| [SOC-007 / #59](https://github.com/chansuke/socium/issues/59) | message_start/stop、block index/type/stop、stop_reasonを検証。不完全EOF、max_tokens、未知/重複sequenceからtoolを実行しない。SSE/response byte上限とtext/thinking/tool/usageの復元テストを追加。 |
| [SOC-008 / #60](https://github.com/chansuke/socium/issues/60) | dry-run/applyを同一の変更計画へ統一。全path/hunkを先に検証し、曖昧なhunk、move衝突、重複/親子pathを拒否。実行直前の内容照合、完全なbefore/after preview、CRLF、適用済みpathを伴う途中失敗を扱う。 |
| [SOC-009 / #61](https://github.com/chansuke/socium/issues/61) | patchとtext_editorを同一の変更前後記録へ接続。moveを含む操作単位のundo、外部編集の検出、失敗時に消費しない履歴、未完操作の明示的復旧を実装。旧履歴は保存するが安全なafter情報がないundoは拒否。 |
| [SOC-010 / #62](https://github.com/chansuke/socium/issues/62) | staged-onlyを既定にし、任意の明示tracked pathsにも対応。git add -Aを廃止。既存indexとrequested diffを承認表示し、待機中の変化を再検証。混在staged/unstaged/untrackedを保全するテストを追加。 |
| [SOC-011 / #63](https://github.com/chansuke/socium/issues/63) | HTTP/承認/command/searchへcancelを伝搬し、Esc/Ctrl+C、worker panic、Closed channel、終了時cleanupを接続。panicで端末が復元されてUIが壊れないようworkerのhookを分離。完了済み結果は保存して再開可能。 |
| [SOC-012 / #64](https://github.com/chansuke/socium/issues/64) | schema付きの単一snapshotにconversation/summary/eventsを保存。create-new temp→write→sync→rename→directory sync、単一writer OS lock、64 MiB上限、壊れた/未知versionの原本保全を実装。各保存境界への故障注入と別processのlock競合を検証。append JSONLは採用していないため未完tail条件は非該当。 |
| [SOC-024 / #76](https://github.com/chansuke/socium/issues/76) | rg -e/--とJSONでhyphen pattern・colon filenameを扱い、regex/ignore等の異なる無言fallbackを廃止。rgがなければ明示エラー。検索5秒・256 KiB、結果件数、listingの時間/走査量/深さ/件数を制限し、切詰めを表示。Nix package/devShellへrgを追加。 |
| [SOC-038 / #91](https://github.com/chansuke/socium/issues/91) | scripted providerと一時Git repoでread→search→edit→test→通信失敗→restart→resumeを検証。人間のindex/untracked変更、command/API cancel、unknown outcome、保存失敗、二重writer、壊れたstateを回帰試験で確認。 |
| [SOC-039 / #92](https://github.com/chansuke/socium/issues/92) | 正規化後の.git/.sociumを通常編集から保護。dry-run/apply/text_editor/undoで同じ境界を使い、内部symlink aliasも拒否。明示承認されたhost commandの権限とは分けて説明。 |

主要な統合fixtureは `crates/core/src/reliability_tests.rs`。保存途中の故障注入は
`agent/state.rs`、pipe/子孫/シグナルは `process.rs`、stream完了契約は
`providers/anthropic/stream.rs`、worker切断はTUI `agent_task.rs` にある。
元レビューの観察用テストは当時の欠陥を示す資料として残し、現在の回帰試験と混同しない。

[運用手順と保証の範囲](daily-reliability.md)を参照。OS sandbox #55、move/delete undoのmetadata保持 #93、安定ID・複数sessionの発展 #65/#78、provider/config/retry/評価などの未完了チケットは維持する。
Web検索は [#90](https://github.com/chansuke/socium/issues/90) に仕様・受け入れ条件を起票済みで、今回のコードには含めない。
