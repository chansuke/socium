# SOC-040: move・deleteのundoでfile modeを保持する

GitHub: [#93](https://github.com/chansuke/socium/issues/93) / 親: [#52](https://github.com/chansuke/socium/issues/52)

## 問題
日常信頼性の実装で、既存ファイルへのatomic replacementは現在のpermissionsを維持する一方、move先の新規ファイルとdelete/moveのundoでは元のUnix executable bit等が履歴に含まれないことを確認した。内容を戻してもスクリプトが実行不能になる場合がある。

## 対応・受け入れ条件
- MutationPlanと共通編集履歴に、変更前後の必要なfile modeを記録する。
- executable scriptのmove、delete→undo、move→undoで内容と実行ビットを保持する。
- 外部chmod後のundoで人間のmetadata変更を無言で消さない。
- metadata復元失敗も適用済み範囲と復旧手順を返す。
- 旧schemaからの移行、未対応OSやACL/所有者/xattrの保証範囲を明記する。

## 依存関係
#60 共通MutationPlan、#61 変更前後とundoの記録。今回の既存内容保護と、中断・再開の基盤を前提に追加する。

