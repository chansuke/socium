# SOC-038: 日常作業の統合受け入れ検証

GitHub: [#91](https://github.com/chansuke/socium/issues/91) / 親: [#52](https://github.com/chansuke/socium/issues/52)

## 目的
読む → 検索する → 既存変更を保った編集 → テスト → 中断 → プロセスを再起動して再開、を一つの受け入れシナリオとして保証する。個別ツールの単体テストだけでは、成功した副作用が失敗ターンから消える問題を見逃す。

## 完了条件
- 実APIや課金を使わない scripted provider と一時Git workspaceで一連の操作を再現する。
- 手動変更、staged変更、untrackedファイルを準備し、それらを誤って破棄・commitしない。
- 大量stdout/stderr、signal終了、timeout、中断の各経路で有限時間内に操作可能になる。
- 編集後のprovider失敗、中断、保存失敗、再起動でも実行済み操作が履歴に残る。不明な副作用は成功・失敗と偽らず、再実行しない。
- 不完全なSSE/tool JSONを実行しない。contextに孤立tool_resultや不正role列を送らない。
- 2プロセスによる状態競合と壊れた状態ファイルは、無言の履歴消失を起こさず明示エラーにする。
- 自動回帰テストをCIで実行し、利用方法と残る制約をREADME/docsに記載する。

## 依存関係・進め方
必須の先行修正: #53 (信頼できる承認), #54 (パス), #56 (process), #57 (履歴), #58 (context), #59 (stream), #60 (patch), #62 (git), #63 (cancel), #64 (storage)、#92 (内部metadataの編集保護)。本チケットは統合検証の完了ゲート。各issueの広い受け入れ条件を満たさない場合は、実装済み範囲と残作業を明示し、安易にcloseしない。
Web検索 #90 は独立した機能追加であり、この信頼性ゲートの前提ではない。


**実装・検証記録**

scripted providerと一時Git repoでread→search→edit→test→通信失敗→restart→resumeを検証。人間のindex/untracked変更、command/API cancel、unknown outcome、保存失敗、二重writer、壊れたstateを回帰試験で確認。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。
