# SOC-029: 配布物のgit・rg依存と対応環境を明確にする

| 項目 | 値 |
|---|---|
| GitHub Issue | [#81](https://github.com/chansuke/socium/issues/81) |
| 親Issue | [改善計画・依存関係 #52](https://github.com/chansuke/socium/issues/52) |
| 起票時ラベル | `review:2026-11-11`、`priority:p2`、`bug`、`area:delivery`、`status:ready` |
| GitHub前提 | なし |
| GitHub後続 | なし |
| 状態 | 未着手 |
| 優先度 | P2 |
| 起票日 | 2026-11-11 |
| 出典 | [詳細レビュー](../review-socium-pi-dsh-2026-11-11.md) — F12 配布／品質基盤 |
| 前提チケット | なし |
| 作業分類 | 実装・検証 |

現在の実行状態はGitHubのopen/closed・状態ラベル・依存関係を確認する。上のラベルは起票時の記録。[運用規約](github-workflow.md) / [機械可読の対応表](github-issues.json)。

**問題と目的**

Nix packageのruntime PATHにgit/rgが含まれず、開発shellでは動くが配布環境で機能や検索意味が変わり得る。

**対応範囲**

- 必須/任意の外部commandと対応OSを定義し、Nix wrapperまたは起動診断へ反映する。
- API/provider設定、最低限の起動手順、sandboxの保証範囲をREADMEに記載する。
- 開発用PATHに頼らない配布物smoke checkをCIに追加する。

**受け入れ条件**

- [ ] 空に近いPATHから配布binaryを起動して、契約上必須のgit/rg操作が動く。
- [ ] 任意依存がない場合、fallbackの機能差または明示エラーを利用者が把握できる。
- [ ] READMEだけで対応providerを設定し、初回起動できる。
- [ ] 実際に検証したOSと未検証/未対応環境を区別して記載する。

**根拠・再現条件**

この項目に対応する独立した観察用再現テストは元レポートにない。以下の検証方法で受け入れ条件を確認し、静的指摘と実測結果を区別する。

**検証方法**

- Nix buildとclean-environment smoke testを行い、利用できないplatformは未検証と記録する。

受け入れ条件を満たした結果・実行コマンド・未検証範囲を完了時に記録する。共通の検証方針は[チケット一覧](README.md)を参照。

**主な関連ファイル**

- [flake.nix](../../flake.nix)
- [README.md](../../README.md)
- [.github/workflows/ci.yaml](../../.github/workflows/ci.yaml)
- [crates/core/src/tools/ripgrep.rs](../../crates/core/src/tools/ripgrep.rs)
- [crates/core/src/tools/git.rs](../../crates/core/src/tools/git.rs)

**対象外・境界**

全OSのinstaller制作や署名配布は含めない。

**完了記録**

2026-11-17（branch `feat/runtime-dependency-check`）。

- Nix package は既に git・rg・bwrap を wrapper の PATH に入れていた。
- 起動診断を追加: tool と同じ探し方（PATH のシステムディレクトリと /usr/bin・/bin）で rg と git を探し、無ければ起動時の警告に、失敗する tool の名前とともに出す。
- 配布物の smoke: headless の実バイナリ試験 `a_missing_ripgrep_is_named_at_startup` が、bwrap と git だけの PATH で起動する。Nix package build は試験を Nix の sandbox で走らせ、そこには /usr/bin が無いので、CI では警告が出る側を確認する。
- README に探す場所と警告を記載。
- 残り: 「実際に検証した OS」の記載。所有者の検証環境が確認できないため未記入（README は Linux のみ対応、macOS・Windows 非対応と記載済み）。

[一覧へ戻る](README.md)

