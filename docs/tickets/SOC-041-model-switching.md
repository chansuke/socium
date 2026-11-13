# SOC-041: model registryと/modelでAnthropic形式の接続先を切り替える

GitHub: [#106](https://github.com/chansuke/socium/issues/106) / 親: [#52](https://github.com/chansuke/socium/issues/52) / ADR: [0006](../adr/0006-モデルを接続形式と接続先の組で登録しmodelコマンドで切り替える.md)

## 目的

`models.toml` に登録したモデルを `/model` で会話中に切り替え、選択を会話とともに保存する。第一段階として Anthropic Messages 形式の接続先（Anthropic、DeepSeek と Z.ai の Anthropic 互換 endpoint、loopback の互換 server）に対応する。OpenAI 形式は SOC-016、SOC-042 で扱う。

## 対応範囲

- `$XDG_CONFIG_HOME/socium/models.toml`（`SOCIUM_MODELS_FILE` で差替え）を読み、組み込みの `anthropic` provider と `opus`／`sonnet`／`haiku` に重ねる。provider は wire・base_url・api_key_env・auth、モデルは alias・provider・id・features・context_window・max_output_tokens・compact_model を持つ。
- 設定を起動時に検証する。不明な wire、Phase 1 で未実装の wire、https でない非 loopback の base_url、存在しない provider／compact_model、alias の重複を、最初の有料 request より前に示す。
- Anthropic provider の URL と認証ヘッダーを設定から組み立てる。thinking・effort・eager_input_streaming・beta header・text editor の型は、モデルの features に挙げたときだけ送る。features に `anthropic_text_editor` が無いモデルには、text editor を同名・同入力の JSON schema tool として渡す。
- `AgentService` に利用可能なモデルの一覧と切り替えを追加する。切り替え時にキーを検査し、履歴の thinking ブロックを取り除いて保存する。
- 選択した alias を `conversation.json` に保存し、起動時は保存値 → `MODEL` → `default` → `opus` の順で決める。
- compaction は使用中モデルの `compact_model`（未設定なら同じモデル）へ送る。`COMPACT_MODEL` は廃止し警告する。
- TUI: `/model` で一覧を composer の位置に出し、`/model <alias>` で直ちに切り替える。ヘッダーのモデル表示を `AgentService` から取る。切り替えを transcript に記録する。

## 受け入れ条件

- [x] `models.toml` が無いとき、従来と同じ request を Anthropic に送る。
- [x] DeepSeek の Anthropic 互換 endpoint 向けのモデルで、Anthropic 固有の項目と beta header を送らず、text editor を JSON schema tool として送る。
- [x] bearer と x-api-key のどちらの認証も、キーを sensitive なヘッダーとして送る。キーが無い provider のモデルは一覧で選べず、切り替えは理由付きで失敗する。
- [x] 切り替え後の最初の request に、切り替え前の thinking ブロックが含まれない。
- [x] 選択が再起動後も保たれ、登録から消えた alias は警告して既定へ戻る。
- [x] 実行中の `/model` は拒否される。
- [x] 要約が使用中モデルの compact_model に送られる。
- [x] 設定の解析・検証・優先順位・request body を、process の環境変数を書き換えずに試験する。

## 対象外

OpenAI 形式の wire（SOC-016、SOC-042）、モデル一覧の自動取得、料金の表示、プロジェクト単位の models.toml。

## 依存・関連

必須の前提は無い。SOC-015 の設定一元化の一部（provider・endpoint・model・context/output・compaction）を先行して実装する。timeout などの残りは SOC-015 に残す。SOC-014 の tool 投影（text editor の JSON schema 化）の一部を先取りする。

## 検証・完了記録

2026-11-13 実装（`8ca5c42`、続く commit で live 試験・context 警告・alias 補完を追加）。

- 単体試験: `nix develop -c cargo test --workspace`（core 195、tui 66 通過、live 3 件は ignore）、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo fmt --all --check`。
- 実 API（`crates/core/src/live_tests.rs`、`SOCIUM_LIVE_MODEL=<alias> cargo test -p socium-core live_ -- --ignored`）: Anthropic の `haiku`（全 features）と、features を空にした `claude-haiku-5-5`（互換 endpoint と同じ body、JSON schema の text editor）で、テキスト応答・usage・`read_file` 呼び出し・text editor 呼び出しの 3 件が通過。
- DeepSeek（`https://api.deepseek.com/anthropic`、`deepseek-flash`）と Z.ai（`https://api.z.ai/api/anthropic`、`glm-5.3`）は、無効なキーで Bearer 認証まで到達し、`deepseek API error 401 … authentication_error` と `zai API error 401 …` が表示されることを確認。Z.ai は 2026-11-13 に有効な標準 API キー（Standard API Key）で live 試験 3 件が通過し、標準キーが `/api/anthropic` で使えることを確認した。残高が無いと `rate_limit_error` の `[1113] Insufficient balance` が返る。その後、同じキーが `/api/anthropic` と `/api/paas/v4` の両方で `1000 身份验证失败`（公式表では原因の記載なし）を返すようになり、新しいキーを作成して解消した。socium の送り方とは無関係。DeepSeek の有効なキーでの応答は未確認。
- モデル ID は各社文書で確認した。DeepSeek-V4.1-Flash は `deepseek-flash`（`deepseek-v4.1-flash` は 400）、GLM-5.3 は `glm-5.3`（1M context、最大出力 128K、推論は常に有効）。
- 切替先の予算で履歴（既存の要約後、切詰め前）を見積もり、compaction の閾値以上なら警告して TUI が要約を始める。
- `/model <途中>` で Tab を押すと alias を共通接頭辞まで補完し、候補が複数なら状態行に並べる。
- 実行中の `/model` 拒否は既存の「Request already in progress」経路による（新規試験なし）。
- `/model` の一覧を Claude Code に倣い、説明行・番号付きの行・現在のモデルの ✔・各モデルの `description`（models.toml の任意項目、組み込みは既定文）を表示する形にした。数字キー 1-9 で直接選んで確定できる。
