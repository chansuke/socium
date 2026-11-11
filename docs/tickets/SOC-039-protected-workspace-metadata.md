# SOC-039: Git・セッション状態の保護

GitHub: [#92](https://github.com/chansuke/socium/issues/92) / 親: [#52](https://github.com/chansuke/socium/issues/52)

## 問題
日常作業の修正中、通常ファイル用の編集ツールが .git/ と .socium/ も書き換えられることを確認した。会話履歴・承認状態・Git設定をモデルの通常編集から保護しないと、パス境界がworkspace内でも信頼境界を越える。

## 対応・受け入れ条件
- apply_patch、text_editor、undoで、正規化・symlink解決後の .git と .socium 配下への変更を拒否する。
- dry-runにも同じ制約を適用し、通常ファイルから保護領域へのmoveや内部symlink経由の別名を拒否する。
- 拒否前に既存ファイル・履歴へ副作用を与えない。
- host権限で明示承認したrun_commandはOS sandboxではない。保証の範囲を文書化する。

## 依存関係
#54 パス正規化、#60 共通MutationPlan。#91 の既存変更・セッション整合性の検証に追加する。


**実装・検証記録**

正規化後の.git/.sociumを通常編集から保護。dry-run/apply/text_editor/undoで同じ境界を使い、内部symlink aliasも拒否。明示承認されたhost commandの権限とは分けて説明。

検証: Nix環境のfmt、Clippy（warnings deny）、全125テスト（core 111 / TUI 14）が成功。実API・課金なし。実装と受け入れ試験は本変更セット。関連PRはGitHub Issueのclosing PRを参照。[運用・制約](../daily-reliability.md) / [検証対応表](../daily-reliability-validation.md)。Linuxで検証。macOS実行・OS sandboxは未検証/未実装。
