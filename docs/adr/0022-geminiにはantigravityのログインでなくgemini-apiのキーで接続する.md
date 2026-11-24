---
number: 22
title: GeminiにはAntigravityのログインでなくGemini APIのキーで接続する
status: proposed
date: 2026-11-20
links:
- target: 6
  kind: amends
- target: 23
  kind: relates-to
---

# GeminiにはAntigravityのログインでなくGemini APIのキーで接続する

## Context and Problem Statement

Google Antigravity で使える Gemini のモデルを socium からも使いたい。ADR 6 の wire は Anthropic Messages、OpenAI Chat Completions、OpenAI Responses の三つで、Gemini の generateContent API を話せない。Gemini には OpenAI 互換の口もある。ただし thinking の署名 (thought signature) を tool 呼び出しの間で持ち回るには Google 独自の拡張項目が要り、socium の `openai-chat` の wire はそれを扱わない。

Antigravity の Google ログイン (OAuth) を借りて Cloud Code の API を呼ぶ方法もある。OpenCode 向けのプラグインがこの方法を取っていた。しかし Google はこの使い方を規約で禁じており、2026年2月にはプラグインの利用者のアカウントが停止された ([Google AI Developers Forum](https://discuss.ai.google.dev/t/account-banned-after-using-opencode-oauth-plugin-appeal-request/130898)、[プラグインの README](https://github.com/shekohex/opencode-google-antigravity-auth))。Google は、サードパーティのエージェントからは AI Studio か Vertex AI の API キーを使うよう案内している。

定額プランの枠を使うもう一つの方法として、Google 公式の Antigravity CLI (`agy acp`) を Agent Client Protocol (ACP) の agent として起動するものがある。T3 Code はこの方法を取り、agy が自分でファイルを読み書きしてコマンドを実行することを前提にしている ([T3 Code の文書](https://github.com/pingdotgg/t3code/blob/main/docs/user/providers-acp.md))。agy 1.5.13 で次のことを確かめた。

* client が ACP の `terminal` と `fs.writeTextFile` を提供すると宣言しても、agy はコマンドとファイルの書き込みを自分で実行し、client に任せなかった。
* コマンドの前には許可を求めたが、ファイルの書き込みはワークスペースの外でも許可を求めなかった。
* 非特権の user namespace を AppArmor が制限するホスト (`kernel.apparmor_restrict_unprivileged_userns = 1`) では、agy の terminal sandbox が使えず、`--sandbox` を付けても何も制限されなかった。agy はこれを log に書くだけで、コマンドは `~/.ssh` を一覧でき、ホームの下にファイルを作れた。文書も、sandbox が使えないときは `--sandbox` が効かないとしている ([Terminal sandbox](https://antigravity.google/docs/sandbox))。
* OAuth のトークンは、D-Bus があればデスクトップの鍵束 (Secret Service) に、なければ agy のホームの下のファイルに置かれる。どちらでも、agy が実行するコマンドからトークンに届く。

規約の上での扱いも定まらない。ACP の文書は「Any ACP client can launch Antigravity CLI」とする ([ACP](https://antigravity.google/docs/ide/extensions/acp/))。一方で FAQ は、Claude Code や OpenCode のようなサードパーティのソフトから Antigravity のログインを使うことを規約違反とし、`agy` をサードパーティのコーディングエージェントやアプリの中に包むことは個人向けプランで認めていない ([FAQ](https://antigravity.google/docs/faq))。追加規約の 6 条は、サードパーティのソフトでサービスにアクセスすることを違反とし、アカウントの停止の理由になるとする ([Additional Terms](https://antigravity.google/terms))。フォーラムには一人で対話的に使う ACP の client は許されるという回答があるが、回答者が Google の人かは確かめられず、別の回答と食い違う ([Google AI Developers Forum](https://discuss.ai.google.dev/t/personal-app-using-the-official-agy-acp-binary-with-google-ai-pro-permitted/198889))。socium はコーディングエージェントなので、FAQ が禁じる側に入りうる。

## Decision Drivers

* 利用者のアカウントを止められうる使い方、規約の上で許されるかが定まらない使い方をさせない。
* Gemini の thinking と tool 呼び出しを、ほかの wire と同じ程度に扱える。
* OpenCode Zen など、generateContent の形で Gemini を出す別の接続先にも使える。
* コマンドを通信なし、ホームなしの sandbox で実行するという socium の約束を、どのセッションでも崩さない。

## Considered Options

* `wire = "google"` を足し、generateContent に API キーで接続する
* Antigravity の OAuth で Cloud Code の API に接続する
* agy を ACP の agent として起動する
* Gemini の OpenAI 互換の口を `openai-chat` で使う

## Decision Outcome

Chosen option: 「`wire = "google"` を足す」。規約の内側で使え、thought signature を履歴に残して次の request に送り返せる。

* provider の `base_url` は版の路を含める (`https://generativelanguage.googleapis.com/v1beta`)。request は `{base_url}/models/{id}:streamGenerateContent?alt=sse` に送る。
* キーは既定で `x-goog-api-key` header で送り、`auth = "bearer"` も選べる。
* feature は `effort` だけを受け付け、thinking の水準として送る。
* thought signature は、書いた provider にだけ送り返す (ADR 14)。

### Consequences

* Good, because Gemini を API キー 1 つで使え、OpenCode Zen の Gemini の口にも同じ wire で届く。
* Bad, because Antigravity の定額プランの枠と、プランに含まれる Claude や GPT は使えず、API の従量課金になる。
* Bad, because wire が四つになり、provider の実装と検査が一つ増える。

### Confirmation

provider の単体テストで、scripted server に対して、request の形 (tool 呼び出しと結果、送り返す thought signature)、text と thinking と function call の stream、`finishReason` と usage の対応、認証の header、API のエラーを確かめる。registry のテストで、`wire = "google"` の解釈と、受け付けない feature の拒否を確かめる。実際の API での確認は `SOCIUM_LIVE_MODEL` の live テストで行う。

## Pros and Cons of the Options

### `wire = "google"` を足し、API キーで接続する

* Good, because 規約に沿い、thought signature を扱える。
* Bad, because 新しい wire の実装が要る。

### Antigravity の OAuth で接続する

* Good, because Antigravity の定額プランの枠で使える。
* Bad, because Google の規約に反し、利用者の Antigravity と Gemini CLI のアカウントが止められうる。
* Bad, because 公開されていない API に頼るので、予告なく壊れる。

### agy を ACP の agent として起動する

* Good, because 改変していない公式の binary を通して、定額プランの枠を使える。
* Bad, because socium のようなサードパーティのコーディングエージェントから使うことを FAQ と追加規約が禁じていると読め、許されるかが定まらない。利用者のアカウントが止められうる。
* Bad, because agy はコマンドとファイルの書き込みを自分で実行するので、socium の sandbox と承認に通せない。ホストでそのまま動かせば、sandbox の外で実行する経路を作ることになる。
* Bad, because agy を bwrap に入れても agy 自身に通信が要り、agy が実行するコマンドにも通信と agy のトークンが届く。通信を Google の送り先に絞っても、ワークスペースの内容をそこへ送れる。
* Bad, because agy は自分の loop、tool、圧縮、サブエージェントを持つので、socium が加えるのは表示とコマンドの承認だけになる。そのために ACP の client、通信を許す別の sandbox、書き込みを隔離して差分を反映する仕組みが要り、agy の版が上がるたびに境界を確かめ直すことになる。

### Gemini の OpenAI 互換の口を使う

* Good, because 実装が要らない。
* Bad, because thought signature を持ち回るには `openai-chat` に Google 独自の拡張を足すことになり、互換の口のための wire に一社の都合が入る。

## More Information

[ADR 6](0006-モデルを接続形式と接続先の組で登録しmodelコマンドで切り替える.md) の wire に一つ足す。thinking を送る先の制限は [ADR 14](0014-応答ごとに出所を記録しthinkingは読める送り先にだけ送る.md) による。API の仕様は [generateContent の reference](https://ai.google.dev/api/generate-content) による。

Google がサードパーティのコーディングエージェントから ACP で agy を使うことを個人向けプランで認めると明記し、かつ agy が ACP の client 側の `terminal` と `fs.writeTextFile` を使うようになれば、agy を ACP の agent として起動する選択肢を考え直す。後者が満たされれば agy の tool を socium の sandbox と承認に通せる。そのときは、通信と認証を agy 本体だけに限れるかも確かめる。
