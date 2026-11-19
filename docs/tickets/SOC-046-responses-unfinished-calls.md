# SOC-046: Responses API で終わっていない tool 呼び出しを実行しない

GitHub: なし (2026-11-19 の公開前レビューで発見)

## 問題
OpenAI Responses の stream で `response.completed` が届くと、`response.output_item.done` を受けていない item も終わったものとして扱っていた。互換 server や中継が item の終わりを送らなかった場合、途中で切れた引数のまま tool を実行しうる。

## 対応・受け入れ条件
- `response.completed` の時点で終わっていない `function_call` があれば、その応答を stream のエラーにする。tool は実行しない。
- item の終わり (`response.output_item.done`) は、始まりと同じ種類のものを 1 回だけ認める。種類の違う終わりと 2 回目の終わりは、message、reasoning、function_call のどれでも応答をエラーにする。追跡しない種類の item は扱わない。
- 呼び出しの終わりの `status` は、ないか文字列の `completed` のときだけ認める。
- 終わった item に届いた delta (text、reasoning の要約、引数) はエラーにする。
- 終わりの `arguments` は必須にしない。互換 server が delta だけで送る場合に備える。
- `response.incomplete` で切れた応答はこれまでどおり扱う。
- 応答が終わった後 (`response.completed` か `response.incomplete` の後) に届いた `response.*` イベントはすべてエラーにする。2 回目の終わりで、安全でない止まり方を completed に変えさせない。
- item の ID は照合しない。守るのは途中で切れた stream で、provider 自身はどんな tool 呼び出しでも送れる。同じ ID の呼び出しが 2 つ届いた場合は agent loop が拒否する。終わった後に item の終わりが届いても、検査をすり抜けさせない。

## 完了記録

2026-11-19 完了。

- 試験: `crates/core/src/providers/openai_responses/stream.rs` の `a_completed_response_with_an_unfinished_call_fails` (引数は届いたが item の終わりが届かない呼び出しでエラーになり、エラーに tool 名が出る) 、`a_call_ended_by_another_item_or_left_incomplete_fails` (別の種類の item や `status: incomplete` で終わった呼び出しがエラーになる)、`a_call_changed_after_it_ended_fails` (終わった呼び出しへの delta と 2 回目の終わりがエラーになる)、`text_or_reasoning_after_its_item_ended_fails` (終わった message と reasoning への delta がエラーになる)、`output_after_the_response_ended_fails` (`completed` の後に届く item の終わりと text delta、`incomplete` の後の `completed` がエラーになる)。`a_call_ended_by_another_item_or_left_incomplete_fails` は `status: in_progress`、数値の `status`、message の位置に届いた呼び出しの終わりも確かめる。
