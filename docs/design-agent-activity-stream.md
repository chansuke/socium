# 設計: Activity ストリーム（LLM・エージェントの内部状態の可視化）

| 項目 | 値 |
|---|---|
| 状態 | 決定済み・実装中 |
| ADR | [5](adr/0005-実行の事実から導いたシグナルでエージェントの状態をactivity欄に示す.md)、[2](adr/0002-activity欄でllmの思考とエージェントの実行を構造化したイベントとして表示する.md)、[3](adr/0003-adaptive-thinkingの表示を既定でsummarizedにしthinking-budget-tokensを廃止する.md)、[4](adr/0004-activityの記録をセッション単位のjsonlに保存し起動時に再生する.md) |
| 作成日 | 2026-11-12 |
| 関連チケット | [SOC-020](tickets/SOC-020-usage-and-observability.md)、[SOC-026](tickets/SOC-026-named-sessions-and-replay.md)、[SOC-027](tickets/SOC-027-tui-layout-and-output.md)、[SOC-031](tickets/SOC-031-headless-jsonl.md) |

## 1. 目的

Socium の実行中、Activity 欄に「モデルが今何を考え、何をしようとしていて、結果どうなったか」を
時系列で読みやすく流す。会話欄（transcript）は人間との対話に専念させ、Activity 欄を
エージェントの内部ログを読むための専用画面にする。これを Socium の差別化点とする。

到達目標は次の 4 点。

1. **思考が見える**: reasoning の要約と、tool 呼び出しの合間の progress update がリアルタイムに流れる。
2. **意図が見える**: tool 呼び出しの引数がストリーム中に組み上がっていく様子を見られる（何を読もうとしているか、どのコマンドを打とうとしているか）。
3. **結果と代償が見える**: 各 round の stop reason、token 使用量（cache 含む）、TTFT、所要時間、tool の成否と所要時間。
4. **生データに降りられる**: 整形表示から、選択した round の生の SSE event（JSON）へ切り替えられる。

### 「LLM の気持ち」をどこまで出せるか

API は生の chain of thought を返さない。返るのは次の 3 種類で、これが表示できる上限になる。

| 種類 | 取得方法 | 内容 |
|---|---|---|
| reasoning 要約 | `thinking.display: "summarized"` | 思考の要約。progress update と混在して返る |
| progress update | `thinking.display: "updates"`（beta `thinking-display-updates-2026-08-18`） | tool 呼び出し直前の「何が分かったか・次に何をするか」の短文。reasoning は隠れる |
| 応答本文・tool 入力 | 常に | `text` block と `tool_use` の引数 |

どの display でも思考の量と課金は変わらない（表示の有無だけが変わる）。
それ以上の「気持ち」は、実行事実から導出するシグナル（§6）で補う。

## 2. 現状と問題

| # | 事実 | 場所 | 影響 |
|---|---|---|---|
| 1 | 既定モデル Claude Opus 5.5 では thinking の display が既定で `"omitted"`。Socium は `thinking` を送っていないため、thinking block の本文は常に空になる | `crates/core/src/providers/anthropic/mod.rs:66` | `ThinkingText` が実質届かず、思考カードがほぼ出ない。progress update も空で、長い tool loop が無言に見える |
| 2 | `THINKING_BUDGET_TOKENS` を設定すると `{type:"enabled", budget_tokens}` を送るが、Opus 5.5 ではこれが 400 になる | 同 `:68-75` | 環境変数を設定すると全リクエストが失敗する（既存バグ） |
| 3 | `input_json_delta` は蓄積されるだけで UI に流れない | `stream.rs` `apply_delta` | tool 引数が完成するまで意図が見えない |
| 4 | round ごとの usage・stop reason・block 境界が progress event にない | `providers/mod.rs` `DeltaEvent`、`lib.rs:68` `AgentProgress` | token/時間の内訳を表示できない |
| 5 | `activity_log` は `Vec<String>` で末尾 N 行だけを表示。スクロール・展開・種類別の装飾ができない | `tui/src/app.rs:29`、`transcript.rs:114`、`view/mod.rs:138` | 情報量を増やしても読めない |
| 6 | Activity 欄は高さ 8 行固定（枠を除くと 6 行）、幅 42% | `view/mod.rs:22-39` | 構造化した表示を入れる余地がない |
| 7 | 未知の content block type（`redacted_thinking`、server tool の結果など）は deserialize に失敗して stream ごとエラーになる | `stream.rs` `StreamContentBlockStart` | 将来の block 追加に弱い（SOC-007 の厳格さとの兼ね合いは §9） |

1 と 2 は Activity の機能以前の問題で、先に直す（§8 Phase 0）。

## 3. 全体構成

```
Anthropic SSE ──► SseParser ──► StreamingMessageBuilder ──► ProviderEvent ─┐
                  (raw JSON を ring buffer へ)                              │
tools/run_command/compaction/approval ─────────────────────────────────────┤
                                                                           ▼
                                                   core: ActivityEvent（provider 非依存・ID/時刻付き）
                                                                           │
                                         ┌─────────────────────────────────┼──────────────────────┐
                                         ▼                                 ▼                      ▼
                                TUI: ActivityModel              JSONL 永続化（Phase 3）   headless JSONL（SOC-031）
                                （turn→round→item の木）
                                         │
                                         ▼
                                  Activity pane 描画
```

原則:

- **core が意味づけ、TUI は表示だけ**: 「この thinking block は progress update か」「この tool は何秒かかったか」は core が決めて event に載せる。TUI と headless が同じ事実を返す（SOC-031 の受け入れ条件と一致）。
- **既存 `AgentProgress` は段階的に置き換える**: いきなり消さず、`ActivityEvent` を追加して TUI を移行し、最後に `AgentProgress` の重複部分を削る。承認フロー（`ApprovalRequested`）は制御用なので別に残す。
- **ID で相関させる**: `turn_id`・`round`・`block_index`・`tool_call_id` を全 event に付け、表示・永続化・再生で同じ木を組み立てられるようにする。既存の永続 event（`lib.rs:325-389`）の `turn_id`/`round` と揃える。

## 4. core: イベントモデル

### 4.1 Provider 層（`DeltaEvent` の拡張）

`DeltaEvent` を、block の開始・差分・終了と round の終了まで運ぶ形に広げる。

```rust
pub(crate) enum ProviderEvent {
    BlockStarted { index: usize, kind: BlockKind },
    TextDelta { index: usize, text: String },
    ThinkingDelta { index: usize, text: String },
    ToolInputDelta { index: usize, partial_json: String },
    BlockFinished { index: usize },
    MessageFinished { stop_reason: String, usage: Option<TokenUsage> },
}

pub(crate) enum BlockKind {
    Text,
    Thinking,
    RedactedThinking,
    ToolUse { id: String, name: String },
}
```

- `ToolInputDelta` を出すため、tool 定義に `eager_input_streaming: true` を付ける。部分 JSON は表示専用で、実行に使うのは従来どおり `finish()` で完成・検証した入力だけにする（未完成の入力で tool を走らせない既存の保証は変えない）。
- 生 SSE は `SseParser` の出力を round 単位の ring buffer（上限例: 2 MiB / round、直近 10 round）に保持し、raw view（§5.4）に渡す。

### 4.2 Agent 層（`ActivityEvent`）

実装は `crates/core/src/activity.rs`。JSON では `kind` を平らにし、`type` に snake_case の種類名を入れる。

```rust
pub struct ActivityEvent {
    pub turn_id: usize,
    pub round: usize,          // 1 始まり。compaction など tool loop の外は 0
    pub at_ms: u64,            // turn 開始からの経過（monotonic）
    pub agent: Option<String>, // サブエージェントの event なら、起動した agent 呼び出しの call ID（ADR 8）。
                               // round はサブエージェントごとに 1 から数える。None は JSON に出さない
    pub kind: ActivityKind,
}

pub enum ActivityKind {
    TurnStarted,
    RoundStarted { model: String },
    ThoughtStarted { block: usize },               // 本文のある thinking block だけ
    ThoughtDelta { block: usize, text: String },
    ThoughtFinished { block: usize, role: ThoughtRole, interrupted: bool },
    TextFinished { block: usize, chars: usize },   // 本文は transcript 側
    ToolIntentStarted { block: usize, call_id: String, name: String },
    ToolIntentDelta { block: usize, partial_json: String },   // 表示専用
    ToolIntentFinished { block: usize },
    RoundFinished { stop_reason: Option<String>, usage: Option<TokenUsage>,
                    ttft_ms: Option<u64>, elapsed_ms: u64 },
    ToolStarted { call_id: String, name: String, input_summary: String },
    ApprovalWaiting { call_id: String, tool: String },
    ApprovalResolved { call_id: String, approved: bool, waited_ms: u64 },
    CommandOutput { call_id: String, stdout_bytes: u64, stderr_bytes: u64 },
    ToolFinished { call_id: String, name: String, is_error: bool,
                   elapsed_ms: u64, summary: String },
    CompactionStarted,
    CompactionFinished { covered_messages: usize, used_llm: bool },
    TurnFinished { ok: bool, error: Option<String> },
    Warning { message: String },                   // 記録の失敗など
}
```

TTFT は独立した event にせず、round 内で最初の差分が届いた時刻を `RoundFinished.ttft_ms` に載せる。stream が `message_stop` に届かずに終わった round（失敗・中断・非 stream の provider）は `stop_reason: None` で閉じる。retry（SOC-019）は実装時に種類を足す。生の SSE は `ActivityEvent` に入れず、`AgentProgress::RawProviderEvent` で TUI に渡す（保存しない）。

**reasoning と progress update の区別**: `display: "updates"` では空でない thinking block はすべて progress update。`"summarized"` では両者が混在し、API 上は区別できない。core は「thinking block の直後の block が `tool_use` なら progress update」と、次の block が始まった時点（または message の終了時点）で判定し、`ThoughtFinished.role` に確定値を載せる。確定までは TUI は中立の見た目で表示する。

**中断の番兵**: `max_tokens` 等で止まったとき最後の progress block が `This part of the response was interrupted before it finished.` になる。core で検出して `interrupted: true` を付ける。

### 4.3 リクエスト設定

| 環境変数 | 値 | 既定 | 送信内容 |
|---|---|---|---|
| `THINKING_DISPLAY` | `summarized` / `updates` / `omitted` | `summarized` | `thinking: {type: "adaptive", display}`。`updates` のときだけ beta header を付ける |
| `EFFORT` | `low`〜`max` | 未設定（API 既定 = Opus 5.5 では `medium`） | `output_config.effort` |
| `THINKING_BUDGET_TOKENS` | — | 廃止 | Opus 5.5 では 400 になるため削除し、設定されていたら起動時に警告する |

設定値は SOC-015 の検証済み model config に載せる。thinking block は今どおり signature ごと履歴に戻す（`types.rs:53`）。

## 5. TUI: Activity pane

### 5.1 データモデル

`activity_log: Vec<String>` を `ActivityModel` に置き換える。

```
ActivityModel
 └ Turn (turn_id, 経過, 成否)
    └ Round (round, model, stop_reason, usage, TTFT, elapsed)
       └ Item
          ├ Thought   { role: Reasoning|ProgressUpdate|Pending, text, streaming }
          ├ ToolCall  { name, partial_json, input, status, elapsed, summary, stdout/stderr bytes }
          ├ Approval  { tool, waited }
          └ Note      { compaction / retry / error }
```

各 item は幅ごとの折り返し結果をキャッシュし、テキストが伸びたときだけ再計算する。

### 5.2 表示（mockup）

```
┌ activity · turn 3 · 00:42 ───────────────────────── [f]ocus [r]aw [s]ignals ┐
│ ◆ round 1  opus-5.5 · medium                          TTFT 1.2s · 6.8s      │
│ │ ✦ 思考  ユーザーは Activity 欄の改善を求めている。まず現状の描画経路を      │
│ │         確認し、progress event がどこで文字列化されているか調べる…       │
│ │ → 次は view/mod.rs の draw_activity を読む                    (update)   │
│ │ ⚙ read_file  path: crates/tui/src/view/mod.rs           ✓ 12ms  443 行   │
│ │ ⚙ search_text  pattern: "push_activity"                 ✓ 31ms  14 件    │
│ └ tool_use · in 18.2k (cache 16.0k) · out 412                               │
│ ◆ round 2                                                       streaming… │
│ │ ✦ 思考  push_activity は progress.rs に集中している。構造化するなら▍      │
│ │ ⚙ run_command  cmd: cargo test -p socium-tui▍          (引数を生成中)    │
└─────────────────────────────────────────────────────────────────────────────┘
```

- 記号と色で種類を区別する（思考 = LILAC、progress update = BLUE、tool = PINK、失敗 = 赤系、メタ情報 = MUTED）。既存 `theme.rs` のトークンを使う。
- tool 行は引数の要点だけを出す（`path`、`command`、`pattern` など tool ごとに決めた主キー）。完成前は部分 JSON から主キーを寛容に抜き出して表示する。
- 完了した reasoning は既定で 2 行に畳み、streaming 中の block と選択中の block だけ全文を出す。
- round 末尾に stop reason と token 内訳。usage が未報告なら 0 ではなく `usage n/a` と出す（SOC-020 の方針）。

### 5.3 操作

| キー | 動作 |
|---|---|
| `Ctrl+A` | Activity pane にフォーカス／composer に戻る |
| `↑` `↓` / `PgUp` `PgDn` | item 単位で選択・スクロール。末尾にいるときだけ自動追従する |
| `Ctrl+L` / `Home` `g` | 先頭の item へ移動 |
| `Enter` | 選択 item の展開／折り畳み（tool なら完成した入力 JSON と結果の先頭） |
| `r` | 選択 round の生 SSE event 表示に切り替え |
| `z` | Activity pane を全画面化（transcript と入れ替え） |

### 5.4 Raw view

選択 round の SSE event を 1 行 1 event の整形 JSON で表示する。`ping` は既定で隠す。
表示前に制御文字を除去する（`api_error_message` と同じ方針で端末 escape を通さない）。
request header は保持しないので API key が出る経路はない。

### 5.5 レイアウト

- 幅 ≥ 140 列: 右側に Activity を縦長カラムで常時表示（transcript 60% / activity 40%）。last command は Activity 内の tool item 展開に吸収する。
- それ未満: 現在の下段配置を維持し、高さを画面に応じて 8〜40% で可変にする。`z` で全画面化。
- 80×24 でも composer と transcript が見えることを TestBackend で確認する（SOC-027 の受け入れ条件）。

### 5.6 transcript との役割分担

思考は Activity 欄だけに出し、transcript の思考カード（`render_thinking_card`）は「思考中…」の 1 行インジケータに縮める。tool の開始・終了を transcript に `System` 行として二重に出している現状（`progress.rs:54-70`）も Activity に一本化する。

## 6. 導出シグナル（Phase 4）

判定の規則は [ADR 5](adr/0005-実行の事実から導いたシグナルでエージェントの状態をactivity欄に示す.md) で確定した。消費は context 残量ではなく、turn ごとの出力 token の合計として turn の見出しに出す。

生ログの上に、実行事実から機械的に導出した状態を載せる。推測で人格を与えるのではなく、根拠となる事実をクリックで辿れることを条件にする。

| シグナル | 判定 | 表示例 |
|---|---|---|
| フェーズ | 直近 tool の種類（read/search → 調査、edit/patch → 編集、run_command → 検証） | `調査中` `編集中` `検証中` |
| 堂々巡り | 同じ path を 3 回以上読む／同じコマンドを入力変化なく再実行 | `⟲ 同じファイルを 3 回読んでいる` |
| 回復 | tool 失敗の直後の round で別アプローチの tool を呼んだ | `↺ 失敗から回復: patch → text_editor` |
| 沈黙 | progress update も text もない tool round が連続 N 回 | `… 5 round 無言` |
| 燃焼 | turn 内の累積 output token と context 残量の推移 | `context 残り 41%（この turn で -12%）` |

「沈黙」は Opus 5.5 の migration guide にある、約 5 回の無言 tool round 後に turn-scoped system message で更新を促す手法と接続できる（別途検討。履歴は append-only を守る）。

## 7. 性能・安全

- delta は 1 秒に数十件届く。core→TUI の channel はそのまま流し、TUI 側で描画を 1 frame（~16ms）単位に間引く。
- UI が保持するテキストは item ごとに上限（例: 64 KiB）を設け、超過分は「… n KiB 省略」とする。完全な思考は会話履歴（thinking block）に残っている。
- 永続化する activity は tool 結果の本文を含めず、要約と byte 数だけにする（secret 混入面を広げない）。
- モデル出力・tool 出力・生 SSE はすべて表示前に制御文字を除去する。

## 8. 実装計画

各 Phase を独立した PR にする。

| Phase | 内容 | 主な変更 | 検証 |
|---|---|---|---|
| 0 | thinking 設定の修正 | `THINKING_BUDGET_TOKENS` 廃止、`adaptive` + `display`・`effort` 設定、`updates` の beta header | request body の単体テスト。実 API で thinking block に本文が入ることを手動確認 |
| 1 | core イベントモデル | `ProviderEvent`・`ActivityEvent`、block 境界・tool 入力差分・round usage/stop reason・時刻、progress update 判定、`eager_input_streaming` | SSE fixture → event 列の単体テスト（updates/summarized/中断番兵/tool 並列） |
| 2 | TUI Activity pane | `ActivityModel`、描画、フォーカス・スクロール・展開、可変レイアウト、transcript の二重表示解消 | TestBackend で 80×24 / 140×40、streaming 途中、失敗、usage 未報告 |
| 3 | Raw view と永続化 | SSE ring buffer、`r` 表示、activity JSONL（`.socium` 配下、session 単位）、起動時の過去 turn 再生 | 保存→再読込で同じ木になること、制御文字除去 |
| 4 | 導出シグナル | §6 の判定と表示 | 判定関数の単体テスト（固定 event 列） |

Phase 1 の `ActivityEvent` はそのまま SOC-031 の headless JSONL の event schema の候補になる。

## 9. 検討事項

- **未知 block の扱い**: SOC-007 で stream を厳格に検証している。未知 block を黙って捨てると履歴の往復で thinking の検証（preserved thinking）を壊しうるため、「生 JSON のまま保持して送り返す」か「従来どおりエラー」かを Phase 1 で決める。`redacted_thinking` は型（`ContentBlock::RedactedThinking`）を追加して保持した。`data` は変更せず履歴に保存して送り返し、Activity には本文も Raw view の `data` も出さない（Raw view では `[encrypted, N bytes]` に置き換える）。他の未知 block は従来どおりエラー。
- **要約後の thinking**: 自前の要約や budget trimming で古い turn を置き換えると、残した turn の thinking は prefix が変わり無効になる（Anthropic の prefix check）。要約ごとに `thinking_valid_from` を記録し、それより前に生成された残存 turn の `thinking` / `redacted_thinking` は送らない。要約後に生成された thinking は送り返す。raw 履歴からは削除しない。
- **summarized での区別精度**: 「直後が tool_use なら progress update」は近似。誤判定しても表示の色が変わるだけで、実行には影響しない。
- **他ツールとの比較**: Claude Code / Codex の現行表示は実装前に実機で確認し、差分を README の訴求に使う。

## 10. 決定事項

2026-11-12 に次のとおり決め、ADR に記録した。

1. `THINKING_DISPLAY` の既定は `summarized`（[ADR 3](adr/0003-adaptive-thinkingの表示を既定でsummarizedにしthinking-budget-tokensを廃止する.md)）。
2. 思考は Activity 欄だけに出す（[ADR 2](adr/0002-activity欄でllmの思考とエージェントの実行を構造化したイベントとして表示する.md)）。
3. フォーカスは `Ctrl+A`、幅 140 列以上で右カラムに置く（ADR 2）。
4. Activity を JSONL に保存し起動時に再生する（[ADR 4](adr/0004-activityの記録をセッション単位のjsonlに保存し起動時に再生する.md)）。
