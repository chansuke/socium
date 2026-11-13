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

未実施。
