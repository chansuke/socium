# SOC-037: DeepSeek web_search

GitHub: [#90](https://github.com/chansuke/socium/issues/90) / 親: [#52](https://github.com/chansuke/socium/issues/52)

## 目的

DeepSeek Search APIを独立した `web_search` toolとして追加し、Claudeを含む選択中のLLMから最新のWeb情報を取得できるようにする。LLM providerをDeepSeekに変更することは前提にしない。

## 公式仕様

確認日: 2026-11-11。出典: https://api-docs.deepseek.com/guides/search/

`POST https://api.deepseek.com/v1/tools/search`、Bearer認証、JSON。検索語は1〜128 Unicode文字（空白のみ不可）、countは1〜50。検索構文（site:、lang:、期間等）を改変しない。通常の0件は成功であり、通信失敗と区別する。

## 対応範囲

- `DEEPSEEK_API_KEY` と有効化設定を注入可能な設定へ持たせ、credentialをtool schema・model・履歴・エラーへ出さない。
- tool引数はqueryと有界なcountを基本にし、need_contentは既定false。ori_question/localizationの自動送信はしない。
- title/url/summary/snippet/dateを出典付きで返す。本文・件数・総bytes・処理時間に上限を設け、切詰めを明記する。
- URLと出典を回答に残せるよう、tool結果とTUIに検索元を保持する。外部本文の命令をproject instructionsとして扱わない。
- HTTP mockでエラー分類、timeout/cancel、有界retryを試験する。408/429/5xxだけを設定上限内でretryし、401/402等は設定・残高の問題として表示する。

## 受け入れ条件

- [ ] Anthropicのagent loopからweb_searchを呼び、DeepSeekの検索結果を使って出典URLを含む回答を生成できる（providerは変更しない）。
- [ ] 日本語を文字数で検証し、空白query・範囲外countを送信前に拒否する。検索演算子を保持する。
- [ ] 成功・0件・401・402・429・5xx・不正JSON・巨大response・timeout・中断をmockで検証する。
- [ ] キー未設定時の有効化可否を明示し、キー・Authorization・機密ローカルcontextがログやqueryへ自動混入しない。
- [ ] retry回数とレスポンス上限が制御され、中断後の追加requestを止められる。
- [ ] 有料APIの利用と検索サービスへのquery送信をREADMEで説明する。実API試験は明示的に実行し、通常CIでは費用を発生させない。

## 依存・関連

独立したHTTP clientとmock fixtureの設計は着手可能。#63（中断）、#67（設定）、#75（構造化出力）の契約と統合時に整合させる。#68（第二LLM provider）は必須前提ではない。

## 対象外

このIssueはSearchを対象とする。Fetch API（任意URLのMarkdown取得）は必要性を評価して別Issueに分ける。ブラウザー操作、スクレイピング、検索APIの大量自動利用は含めない。

## 検証・完了記録

未実施。実装PR、mock試験結果、実API確認の有無を記録して完了する。


