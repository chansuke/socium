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

2026-11-17 実装（branch `feat/web-search`）。仕様は https://api-docs.deepseek.com/guides/search/ で 2026-11-17 に再確認した。

- config.toml の `[web_search] enabled = true` と `DEEPSEEK_API_KEY` が揃ったときだけ `web_search` tool を提供する。既定は無効。キーが無ければ提供せず、起動時に理由を出す。`base_url` は https（loopback のみ http）。model からは書けない設定ファイル（ADR 12）。
- 送るのは model が書いた query と count だけ。`ori_question`・`localization`・`need_content` は送らない。query は文字数で 1〜128、空白のみ不可、count は 1〜50（既定 8）を送信前に検証し、検索演算子はそのまま送る。
- 結果は番号付きで title・URL・日付・summary（800 字まで）を返し、全体を 24,000 字に収め、収まらない件数を明記する。web の文章であり指示ではないと結果に書く。0 件は失敗ではない。
- 再試行は 408・429・5xx と通信失敗だけを最大 3 回（1 秒・2 秒の間隔）。401 は鍵、402 は残高の問題として再試行せずに伝える。4 MiB を超える応答と不正な JSON は使わない。1 回 30 秒で打ち切り、中断すると以降の request は送らない。キーは sensitive header で送り、Debug 出力・エラーに出さない。
- サブエージェント: worker は使える。読み取り専用の explorer には渡さない（外部へ送るため）。
- 試験（mock）: 成功（日本語と演算子をそのまま送る）、0 件、401、402、429 後の成功、5xx の再試行上限、不正 JSON、巨大応答、中断、入力検証、キー未設定、設定の検証、agent loop から呼んで結果の URL が次の request に入ること。
- 実 API: `live_web_search`（`--ignored`）を用意。実行はしていない（未確認）。
- 範囲外: Fetch API、30 秒の timeout そのものの試験（中断の試験で代替）。


