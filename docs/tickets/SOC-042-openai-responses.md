# SOC-042: OpenAI Responses APIでGPT-5のreasoningをtool往復をまたいで保つ

GitHub: [#107](https://github.com/chansuke/socium/issues/107) / 親: [#52](https://github.com/chansuke/socium/issues/52) / ADR: [0006](../adr/0006-モデルを接続形式と接続先の組で登録しmodelコマンドで切り替える.md)

## 目的

SOC-016 の Chat Completions では、GPT-5 の reasoning は tool の往復ごとに捨てられる。`openai-responses` wire を追加し、暗号化された reasoning item を同じ turn の tool 往復で送り返して、GPT-5 の推論を保つ。

## 対応範囲

- models.toml の `wire = "openai-responses"` を受け付け、`/v1/responses` に stream で接続する。
- message・tool call・tool result・reasoning item を SOC-014 の共通表現と相互に変換する。reasoning item は `store: false` と `include: ["reasoning.encrypted_content"]` で受け取り、同じモデルへの request にだけ送り返す。
- reasoning の要約を Activity の thinking として表示する。`reasoning.effort` を features で制御する。
- usage（cached・reasoning tokens を含む）、終了理由、HTTP エラーを共通表現へ変換する。

## 受け入れ条件

- [ ] mock endpoint で、tool の往復を含む turn が完了し、二回目以降の request に前の round の reasoning item が含まれる。
- [ ] モデルを切り替えたとき、別モデルの reasoning item を送らない。
- [ ] stream の途中失敗、incomplete、拒否、usage を正しく変換する。
- [ ] 実 API の試験は明示実行とし、通常 CI で費用を発生させない。

## 対象外

server 側の会話状態（`previous_response_id`、`store: true`）、組み込みの hosted tool（web search、code interpreter）。

## 依存・関連

前提: SOC-014（共通表現）、SOC-016（OpenAI 系の認証・エラー・stream 基盤）、SOC-041（registry と切り替え）。

## 検証・完了記録

2026-11-14 実装（branch `feat/openai-responses-wire`、`feat/openai-chat-wire` の上に積む）。きっかけは、`gpt-6-astra` を `openai-chat` で使うと「Function tools with reasoning_effort are not supported for gpt-6-astra in /v1/chat/completions」の 400 になったこと。

- `wire = "openai-responses"`（`crates/core/src/providers/openai_responses/`）。`{base_url}/responses` へ Bearer 認証・stream・`store: false`・`max_output_tokens` で送る。function tool は `strict: false` を明示する（schema が strict の条件を満たさないため）。
- `effort` feature を推論モデルの印とし、`reasoning.summary: "auto"`、`include: ["reasoning.encrypted_content"]`、`EFFORT` を `reasoning.effort` として送る。feature の無いモデルには reasoning 系の項目を送らない。
- reasoning item は core の `Thinking` ブロックに載せる（`thinking` に要約、`signature` に送り返す item の JSON）。`store: false` では item が保存されないため、`id` と `status` は除いて送り返す。モデル切り替え時は既存の `strip_thinking` が取り除くので、別モデルの reasoning item は送らない。JSON でない signature（Anthropic の thinking）は送らない。
- 履歴: tool 呼び出しは `function_call`、結果は `function_call_output`（エラーは `Error: ` を前置）。assistant の文には `phase` を付け、tool 呼び出しと同じメッセージなら `commentary`、それ以外は `final_answer`。
- stream: `output_item.added`/`done` で block を開閉し、`done` の内容（arguments、本文、要約）を正とする。`output_text`・`reasoning_summary_text`・`function_call_arguments` の delta を Activity へ流す。usage（cached を cache read、cache_write を cache 書き込みとして）、`response.incomplete`（`max_output_tokens` は出力上限、他は安全でない停止）、refusal、`response.failed`、`error` イベント、completed 前の切断、不正な arguments を検証。
- 試験: 単体試験と loopback の mock HTTP server（認証ヘッダー、tool の往復で二回目の request に reasoning item が含まれること、400 の表示）。core 229、tui 69 通過。
- 実 API（2026-11-14、`gpt-6-astra`、`store: false`）で確認した:
  - `strict: true` は socium の tool schema を 400（`invalid_function_parameters`、`required` に全 property が要る）で拒否する。`strict: false` と省略はどちらも通る。省略時の既定に頼らないよう `false` を明示したままにする。
  - assistant の `phase` は検証される（`bogus` は 400、`commentary`/`final_answer` は通る）。モデル自身の出力 message にも `phase` が付く。
  - reasoning item は `id` を除いても付けたままでも 200 で受け付けられた。送り返すと二回目の入力が約 288 token 増え（一回目の reasoning_tokens は 286）、送らない場合（これも 200）より多いので、推論は入力として使われている。`id` の除去は必須ではないが、保存しない item を参照しない形として残す。
  - reasoning item には `content: []` が付く。要約は `summary` に入る。effort が既定（応答では `medium`）でも、易しい問いでは reasoning item を返さないことがある。
  - usage の `input_tokens_details` に `cache_write_tokens` があり、`input_tokens` はそれと `cached_tokens` を含む。cache 書き込みとして数えるよう変えた。
  - 応答の `reasoning` には既定で `context: "all_turns"` が入っていた。
  - `SOCIUM_MODELS_FILE=models.toml.example SOCIUM_LIVE_MODEL=gpt EFFORT=high cargo test -p socium-core live_ -- --ignored` の 4 件（新しい `live_tool_result_round_trip_completes_the_turn` を含む）が通った。
