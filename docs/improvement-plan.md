# socium 改善計画

Claude Code との比較分析に基づく改善項目を優先度順に整理する。
分析レポート: [analysis-claude-code-vs-socium.md](./analysis-claude-code-vs-socium.md)
補足調査: [analysis-kairos-in-claude.md](./analysis-kairos-in-claude.md)

---

## 高優先度 (1-5)

### 1. ツールモジュール分割

**現状**: `crates/core/src/tools/runtime.rs:1` (2,107行) に全12ツールの実行ロジックが集約 (`crates/core/src/tools/runtime.rs:54-102` ディスパッチ)。
**目標**: ツール種別ごとにファイルを分離し、保守性を向上。

```
crates/core/src/tools/
├── mod.rs            # ToolDefinition, default_tools() — 既存
├── common.rs         # 共有ユーティリティ (パス解決, ファイル読込, glob, 編集履歴)
├── inputs.rs         # 型付き入力構造体 (Task 2 と統合)
├── dispatch.rs       # run_tool_calls_with_progress + matchディスパッチ
├── read_file.rs      # read_file ツール
├── list_files.rs     # list_files ツール
├── search.rs         # search_text + find_symbol
├── patch.rs          # apply_patch (パーサー, ハンク適用, プレビュー)
├── git.rs            # git_branch/status/diff/log/commit
├── command.rs        # run_command (分類, 実行, フォーマット)
└── text_editor.rs    # str_replace_based_edit_tool (view/replace/create/insert/undo)
```

**影響範囲**:
- `crates/core/src/tools/runtime.rs` → 上記ファイルへの分割
- `crates/core/src/tools/mod.rs` に新モジュールの `pub mod` 追加
- `crates/core/src/lib.rs:271` 以降のテストでインポートパス変更

**リスク**: 低。リファクタリングのみで機能変更なし。
**テスト**: 既存テスト (`crates/core/src/lib.rs:303` 以降、約1,780行) がそのまま通ることを確認。

---

### 2. 型付きツール入力

**現状**: ツール入力を `&serde_json::Value` として受け取り、`require_string_field()` (`crates/core/src/tools/runtime.rs:1486`) / `optional_string_field()` (`crates/core/src/tools/runtime.rs:1497`) で手動抽出。
**目標**: ツールごとに `#[derive(Deserialize)]` な入力構造体を定義し、`serde_json::from_value()` でデシリアライズ。

```rust
// 現在 (crates/core/src/tools/runtime.rs:103-116)
pub(crate) fn read_file_from_workspace(
    workspace_root: &Path, input: &Value,
) -> Result<String, String> {
    let input_object = input.as_object()
        .ok_or("read_file tool requires an object input")?;
    let path = require_string_field(input_object, "path", "read_file path")?;
    // ...
}

// 改善後
#[derive(Deserialize)]
struct ReadFileInput { path: String }

pub(crate) fn read_file_from_workspace(
    workspace_root: &Path, input: &Value,
) -> Result<String, String> {
    let input: ReadFileInput = serde_json::from_value(input.clone())
        .map_err(|e| format!("read_file: invalid input: {e}"))?;
    // input.path でアクセス
}
```

**メリット**:
- コンパイル時型チェック
- エラーメッセージの自動生成
- デフォルト値を `#[serde(default)]` で宣言的に指定
- `require_string_field` / `optional_string_field` / `require_i64_field` (`crates/core/src/tools/runtime.rs:1486-1524`) の手動ヘルパーを削除可能

**影響範囲**: `crates/core/src/tools/runtime.rs` (各ツール関数の冒頭部分、12箇所)
**リスク**: 低。serde のエラーメッセージが既存のカスタムメッセージと異なる点に注意。

---

### 3. リトライ & エラー分類

**現状**: API エラーは即座に `Err` として返され、リトライなし (`crates/core/src/providers/anthropic.rs:33-36`)。ストリーミング途中の失敗も未処理 (`crates/core/src/providers/anthropic.rs:39-50`)。
**目標**: 一時的エラー（429 Rate Limit, 529 Overloaded, ネットワークエラー）に対する指数バックオフリトライ。

```rust
// crates/core/src/providers/anthropic.rs に追加
enum ApiErrorKind {
    Transient,   // 429, 529, ネットワーク接続失敗
    StreamBroken, // ストリーミング途中の切断
    Fatal,       // 400, 401, 403, 404
}

const MAX_RETRIES: u32 = 3;
const BASE_DELAY_MS: u64 = 1000;

pub(crate) async fn send_message_with_retry<F>(
    client: &reqwest::Client,
    api_key: &str,
    anthropic_version: &str,
    message_request: &AgentMessageRequest<'_>,
    on_text_delta: &mut F,
    on_retry: &mut impl FnMut(u32, &str),  // リトライ通知コールバック
) -> Result<ProviderResponse, Box<dyn std::error::Error>>
where
    F: FnMut(String) + Send,
{
    for attempt in 0..=MAX_RETRIES {
        match send_message_once(client, api_key, anthropic_version, message_request, on_text_delta).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                let kind = classify_api_error(&e);
                match kind {
                    ApiErrorKind::Fatal => return Err(e),
                    ApiErrorKind::Transient | ApiErrorKind::StreamBroken if attempt < MAX_RETRIES => {
                        let delay = BASE_DELAY_MS * 2u64.pow(attempt);
                        on_retry(attempt + 1, &e.to_string());
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        // StreamBroken の場合、on_text_delta の状態をリセットする必要がある
                        // → 呼び出し元で partial text を破棄するロジックが必要
                    }
                    _ => return Err(e),
                }
            }
        }
    }
    unreachable!()
}

fn classify_api_error(err: &(dyn std::error::Error + 'static)) -> ApiErrorKind {
    // ステータスコードベースの分類
    // reqwest のエラーチェーンからステータスコードを抽出
    // ネットワークエラー (ConnectionReset, Timeout) → Transient
    // チャンク読取エラー → StreamBroken
    // 4xx (429除く) → Fatal
    todo!()
}
```

**ストリーミング途中失敗への対応方針**:
- `StreamBroken` (チャンク読取中のエラー): リトライ時に `on_text_delta` で送信済みの部分テキストを無効化する必要がある
- 方法1: `on_text_delta` にリセットイベントを追加 (`AgentProgress::StreamReset`)
- 方法2: テキストデルタを内部バッファに蓄積し、成功完了後にまとめて通知
- 方法2 を推奨（UIへの影響が小さい）。ただしストリーミング表示のリアルタイム性は低下

```rust
// 推奨: providers/anthropic.rs 内で内部バッファに蓄積
let mut buffered_text = String::new();
let mut emit_text_delta = |text: String| {
    buffered_text.push_str(&text);
};

let response = send_message_once(..., &mut emit_text_delta).await?;
on_text_delta(buffered_text);
return Ok(response);
```

**影響範囲**:
- `crates/core/src/providers/anthropic.rs`: エラー分類とリトライラッパー追加
- `crates/core/src/providers/mod.rs:92-135`: `send_message` にリトライ通知コールバック追加
- `crates/core/Cargo.toml:6`: `tokio` 依存の追加が必要 (`tokio::time::sleep`)
- `crates/core/src/agent/turn.rs:30-45`: リトライイベントの progress 通知

**リスク**: 中。
- ストリーミング途中失敗のリトライはUIステートのリセットが必要で設計が複雑
- `reqwest::Client` のタイムアウト (現在90秒、`crates/core/src/lib.rs:153-155`) との兼ね合い
- 429 の `Retry-After` ヘッダー対応は追加で検討

---

### 4. システムプロンプト強化

**現状**: `crates/core/src/constants.rs:27` に固定文字列 `DEFAULT_SYSTEM_PROMPT` を定義。`crates/core/src/agent/types.rs:15-29` の `AgentMessageRequest` で `system: &'a str` として参照渡し。git状態や日時情報なし。
**目標**: 動的にシステムプロンプトを組み立て、以下の情報を含める:
- git ブランチ名
- git status (短縮形)
- 現在日時
- ワークスペースルートパス
- OS 情報

```rust
// crates/core/src/agent/system_prompt.rs (新規)
use crate::constants::DEFAULT_SYSTEM_PROMPT;
use std::path::Path;
use std::process::Command;

pub(crate) fn build_system_prompt(workspace_root: &Path) -> String {
    let mut parts = vec![DEFAULT_SYSTEM_PROMPT.to_string()];

    parts.push("# Environment".to_string());
    parts.push(format!("- Working directory: {}", workspace_root.display()));
    parts.push(format!("- Platform: {}", std::env::consts::OS));

    // 日時: std::time で UTC を表示、またはフォーマット済み文字列
    // socium-tui は既に time クレートに依存しているため、core 側も time を追加するか、
    // あるいは std::process::Command で date コマンドを呼ぶ軽量な方法もある
    if let Ok(output) = Command::new("date").arg("+%Y-%m-%d %H:%M %Z").output() {
        if output.status.success() {
            if let Ok(date_str) = String::from_utf8(output.stdout) {
                parts.push(format!("- Current date: {}", date_str.trim()));
            }
        }
    }

    // Git情報
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(workspace_root)
        .output()
    {
        if output.status.success() {
            if let Ok(branch) = String::from_utf8(output.stdout) {
                parts.push(format!("- Git branch: {}", branch.trim()));
            }
        }
    }

    if let Ok(output) = Command::new("git")
        .args(["status", "--short"])
        .current_dir(workspace_root)
        .output()
    {
        if output.status.success() {
            if let Ok(status) = String::from_utf8(output.stdout) {
                let status = status.trim();
                if !status.is_empty() {
                    // 長すぎる場合は先頭20行に制限
                    let truncated: String = status.lines().take(20).collect::<Vec<_>>().join("\n");
                    parts.push(format!("- Git status:\n{truncated}"));
                }
            }
        }
    }

    parts.join("\n\n")
}
```

**影響範囲**:
- `crates/core/src/constants.rs:27`: `DEFAULT_SYSTEM_PROMPT` はベース部分として維持
- `crates/core/src/agent/types.rs:15-29`: `AgentMessageRequest` の `system` を `&'a str` → `String` (または `Cow<'a, str>`) に変更
- `crates/core/src/lib.rs:216-223`: `prepare_context_messages` 呼出前に `build_system_prompt` を呼び出し
- `crates/core/src/agent/turn.rs:35`: `AgentMessageRequest` 構築部分の修正
- 新規: `crates/core/src/agent/system_prompt.rs`
- `agent/mod.rs`: `pub mod system_prompt;` 追加

**リスク**: 低。
- `AgentMessageRequest` のライフタイム変更は型を `system: String` にすれば単純化できる
- git コマンドの実行はメッセージ送信ごとに発生（ラウンドあたり1回）。git status は軽量なので許容範囲

---

### 5. 中断サポート (Esc / Ctrl+C)

**現状**: TUI は「esc to cancel」と表示する (`crates/tui/src/main.rs:1000`) が、Esc キーは実際にはリクエストをキャンセルせず `InputMode::Normal` に切り替えるのみ (`crates/tui/src/main.rs:490-497`)。エージェントループ (`crates/core/src/agent/turn.rs:12-74`) にも中断メカニズムなし。
**目標**: Esc キーまたは Ctrl+C で進行中のリクエスト/ツール実行を実際にキャンセルする。

```rust
// 方法: tokio::sync::watch または CancellationToken を使用

// crates/core/src/agent/turn.rs に CancellationToken を追加
use tokio_util::sync::CancellationToken;

pub(crate) async fn run_agent_turn<F>(
    // ... 既存パラメータ ...
    cancel: CancellationToken,  // 追加
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    for round in 0..MAX_TOOL_ROUNDS {
        if cancel.is_cancelled() {
            return Err(Error::other("Request cancelled by user").into());
        }

        // API 呼出を select! でキャンセル可能にする
        let response = tokio::select! {
            result = providers::send_message(...) => result?,
            _ = cancel.cancelled() => {
                return Err(Error::other("Request cancelled by user").into());
            }
        };

        // ツール実行もキャンセルチェック
        if cancel.is_cancelled() {
            return Err(Error::other("Request cancelled by user").into());
        }
        let tool_results = run_tool_calls_with_progress(...);
        // ...
    }
}

// crates/tui/src/main.rs: Esc ハンドラの修正
KeyCode::Esc => {
    if let Some(ref task) = self.pending_task {
        // 実際にキャンセル
        task.cancel_token.cancel();
        self.status = Some("Cancelling...".to_string());
    } else {
        self.input_mode = InputMode::Normal;
        self.status = Some("Press E to resume typing".to_string());
    }
}
```

**影響範囲**:
- `crates/core/Cargo.toml:6`: `tokio-util` 依存追加 (CancellationToken)
- `crates/core/src/agent/turn.rs:12-74`: `run_agent_turn` に cancel パラメータ追加
- `crates/core/src/lib.rs:207-246`: `submit_with_progress` に cancel パラメータ追加
- `crates/tui/src/main.rs:490-497`: Esc ハンドラの修正
- `crates/tui/src/main.rs` の `PendingTask` 構造体に `cancel_token` フィールド追加
- ツール実行中のコマンド (`crates/core/src/tools/runtime.rs` の `Command::new()`) は `kill()` で強制停止が必要

**リスク**: 中。
- ストリーミング中のキャンセルは `reqwest` の応答ストリームを drop することで実現可能
- ツール実行中（特に `run_command`）のキャンセルは子プロセスの kill が必要
- キャンセル後のメッセージ履歴の一貫性に注意（部分的なアシスタント応答を履歴に含めるか）
- `tokio::select!` を使うには `send_message` が async である必要がある（既にasync）

**注意**: 元の計画では低優先度(15)だったが、UI が「esc to cancel」と表示している以上、ユーザー期待との乖離を早期に解消すべき。

---

## 中優先度 (6-10)

### 6. スラッシュコマンド

**現状**: TUI で `/quit`, `/exit`, `/help`, `/clear` のみ (`crates/tui/src/main.rs:237-258` でハードコード)。
**目標**: 以下のコマンドを追加:
- `/compact` — コンテキストの強制圧縮
- `/model` — 現在のモデル表示 / 変更
- `/cost` — セッションのトークン使用量と推定コスト表示
- `/status` — コンテキストメトリクス詳細表示
- `/reset` — 会話リセット（/clearのエイリアスまたは確認付き）

```rust
// crates/tui/src/main.rs (begin_submit メソッド内、crates/tui/src/main.rs:237 以降に追加)
"/compact" => {
    // core 側に force_compact API が必要
    // → AgentService に pub fn compact(&mut self) -> io::Result<()> を追加
    match self.agent.as_mut() {
        Some(agent) => {
            match agent.compact() {
                Ok(metrics) => {
                    self.push_message(Speaker::System,
                        format!("Context compacted. {} messages summarized.", metrics.summarized_messages));
                }
                Err(e) => {
                    self.push_message(Speaker::System, format!("Compact failed: {e}"));
                }
            }
        }
        None => { self.push_message(Speaker::System, "No agent loaded".to_string()); }
    }
    return Ok(SubmitAction::None);
}
"/model" => {
    // 現在のモデル名を表示。変更は /model <name> で対応
    // → AgentService に pub fn model(&self) -> &str と pub fn set_model(&mut self, model: String) を追加
    match self.agent.as_ref() {
        Some(agent) => {
            self.push_message(Speaker::System, format!("Current model: {}", agent.model()));
        }
        None => { self.push_message(Speaker::System, "No agent loaded".to_string()); }
    }
    return Ok(SubmitAction::None);
}
"/cost" => {
    // Task 7 (コスト追跡) の API に依存
    // → AgentService に pub fn usage(&self) -> &TokenUsage を追加
    return Ok(SubmitAction::None);
}
```

**core 側に必要な新 API**:
- `AgentService::compact(&mut self) -> io::Result<ContextMetrics>` — 強制コンテキスト圧縮 (`crates/core/src/agent/context.rs:66-80` の `prepare_context_messages` を直接呼び出し、summary_state を更新)
- `AgentService::model(&self) -> &str` — 現在のモデル名取得
- `AgentService::set_model(&mut self, model: String)` — モデル変更
- `AgentService::usage(&self) -> &TokenUsage` — Task 7 と連携

**影響範囲**:
- `crates/tui/src/main.rs:237-258`: コマンドパーサー拡張
- `crates/core/src/lib.rs`: `AgentService` に `compact()`, `model()`, `set_model()` メソッド追加
- `crates/core/src/agent/context.rs:66`: `prepare_context_messages` を compact 用に呼び出せるようにする

**リスク**: 低〜中。`/compact` と `/model` は core 側の API 追加が必要で、TUI だけでは完結しない。

---

### 7. コスト追跡

**現状**: Anthropic API の `message_delta` イベントに含まれる `usage` フィールドを無視 (`crates/core/src/providers/anthropic.rs:42-46` のストリームループ)。`message_start` の `usage` も未処理。
**目標**: ストリーミングレスポンスからトークン使用量を抽出し、セッション累積値として保持。

```rust
// crates/core/src/providers/anthropic.rs — StreamingMessageBuilder に追加
#[derive(Debug, Clone, Default)]
pub(crate) struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

// StreamingMessageBuilder の apply_event 内で:
// "message_start" → message.usage.input_tokens を取得
// "message_delta" → usage.output_tokens を取得
fn apply_event<F>(&mut self, event: &SseEvent, on_text_delta: &mut F) -> Result<(), ...> {
    match event.event_type.as_str() {
        "message_start" => {
            if let Some(usage) = event.data.get("message")
                .and_then(|m| m.get("usage"))
            {
                self.usage.input_tokens = usage.get("input_tokens")
                    .and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            }
        }
        "message_delta" => {
            if let Some(usage) = event.data.get("usage") {
                self.usage.output_tokens = usage.get("output_tokens")
                    .and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            }
        }
        // ...
    }
}

// crates/core/src/providers/mod.rs — ProviderResponse に追加
pub(crate) struct ProviderResponse {
    pub message: AgentMessageResponse,
    pub streamed_text: bool,
    pub usage: TokenUsage,  // 追加
}

// crates/core/src/lib.rs — AgentService にセッション累積値を追加
pub struct AgentService {
    // ... 既存フィールド ...
    cumulative_usage: TokenUsage,  // 追加
}

impl AgentService {
    pub fn usage(&self) -> &TokenUsage { &self.cumulative_usage }
}

// submit_with_progress 内で累積
// let response = run_agent_turn(...).await?;
// self.cumulative_usage.input_tokens += response.usage.input_tokens;
// self.cumulative_usage.output_tokens += response.usage.output_tokens;
```

**コスト計算** (Anthropic pricing):
```rust
impl TokenUsage {
    pub fn estimated_cost_usd(&self, model: &str) -> f64 {
        let (input_per_mtok, output_per_mtok) = match model {
            m if m.contains("opus") => (15.0, 75.0),
            m if m.contains("sonnet") => (3.0, 15.0),
            m if m.contains("haiku") => (0.25, 1.25),
            _ => (3.0, 15.0), // default to sonnet pricing
        };
        (self.input_tokens as f64 / 1_000_000.0) * input_per_mtok
            + (self.output_tokens as f64 / 1_000_000.0) * output_per_mtok
    }
}
```

**影響範囲**:
- `crates/core/src/providers/anthropic.rs`: `StreamingMessageBuilder` に `usage` フィールド追加、`message_start` / `message_delta` のパース
- `crates/core/src/providers/mod.rs:32-35`: `ProviderResponse` に `usage` 追加
- `crates/core/src/lib.rs:73-86`: `AgentService` に `cumulative_usage` 追加
- `crates/core/src/agent/turn.rs`: `run_agent_turn` の戻り値に usage を含める
- `crates/tui/src/main.rs`: ステータスバーに表示

**リスク**: 低。

---

### 8. 設定ファイル (`.socium/config.toml`)

**現状**: 全設定が環境変数のみ (`crates/core/src/lib.rs:140-151`)。
**目標**: `.socium/config.toml` で以下を設定可能に。環境変数によるオーバーライドを維持。

```toml
# .socium/config.toml

[model]
name = "claude-sonnet-4-5"
max_tokens = 4096
context_window = 200000  # optional override

[provider]
name = "anthropic"
# api_key は環境変数のみ (セキュリティ上、設定ファイルには書かない)

[permissions]
# コマンド実行時の自動承認レベル
# "none" = 全て手動承認, "read-only" = SafeReadOnly自動承認, "workspace" = WorkspaceWrite含む
auto_approve = "read-only"
```

```rust
// crates/core/src/config.rs (新規)
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
pub struct SociumConfig {
    #[serde(default)]
    pub model: ModelConfig,
    #[serde(default)]
    pub provider: ProviderFileConfig,
    #[serde(default)]
    pub permissions: PermissionsConfig,
}

#[derive(Debug, Deserialize)]
pub struct ModelConfig {
    #[serde(default = "default_model")]
    pub name: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    pub context_window: Option<u32>,
}

fn default_model() -> String { "claude-sonnet-4-5".to_string() }
fn default_max_tokens() -> u32 { 1000 }

impl Default for ModelConfig {
    fn default() -> Self {
        Self { name: default_model(), max_tokens: default_max_tokens(), context_window: None }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ProviderFileConfig {
    #[serde(default = "default_provider")]
    pub name: String,
}

fn default_provider() -> String { "anthropic".to_string() }

#[derive(Debug, Deserialize, Default)]
pub struct PermissionsConfig {
    #[serde(default = "default_auto_approve")]
    pub auto_approve: String,
}

fn default_auto_approve() -> String { "none".to_string() }

pub fn load_config(workspace_root: &Path) -> Result<SociumConfig, Box<dyn std::error::Error>> {
    let config_path = workspace_root.join(".socium").join("config.toml");
    if !config_path.exists() {
        return Ok(SociumConfig::default());
    }
    let content = std::fs::read_to_string(&config_path)?;
    let config: SociumConfig = toml::from_str(&content)?;
    Ok(config)
}
```

**影響範囲**:
- `Cargo.toml` (core): `toml` クレート追加
- 新規: `crates/core/src/config.rs`
- `crates/core/src/lib.rs:88-171` (`from_env`): 設定ファイル読み込みを追加、env vars でオーバーライド
- `tools/runtime.rs` のコマンド権限チェック: `auto_approve` 設定を参照

**リスク**: 低。

---

### 9. Extended Thinking 対応

**現状**: Anthropic API の `thinking` パラメータ未対応。`crates/core/src/providers/anthropic.rs:13-30` のリクエストボディ構築に thinking フィールドなし。
**目標**: thinking 有効化時に API リクエスト、ストリーミング応答、TUI表示を対応。

```rust
// crates/core/src/providers/anthropic.rs — リクエストボディ構築に追加
fn build_request_body(request: &AgentMessageRequest<'_>, thinking_budget: Option<u32>) -> Value {
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "system": request.system,
        "messages": serialize_messages(request.messages),
        "stream": true,
    });

    if let Some(budget) = thinking_budget {
        body["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": budget,
        });
    }

    if !request.tools.is_empty() {
        body["tools"] = serialize_tools(request.tools);
    }

    body
}

// crates/core/src/agent/types.rs — ContentBlock に Thinking バリアント追加
pub(crate) enum ContentBlock {
    Text { text: String },
    Thinking { thinking: String },  // 追加
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String, is_error: Option<bool> },
}

// crates/core/src/providers/anthropic.rs — StreamingMessageBuilder で thinking ブロックを処理
// content_block_start で type: "thinking" のブロックを認識
// content_block_delta で thinking テキストを蓄積

// crates/core/src/lib.rs / crates/tui/src/main.rs — thinking 用 progress を追加
enum AgentProgress {
    AssistantText { text: String },
    ThinkingText { text: String },
}

// thinking 予算は max_tokens と別枠で扱う
let effective_output_budget = request.max_tokens + thinking_budget.unwrap_or(0);
assert!(effective_output_budget <= context_budget.context_window_tokens);

// crates/tui/src/main.rs — thinking の表示
// thinking 中は "Thinking..." のスピナーを表示
// thinking 完了後は折りたたみ可能な thinking テキストを表示 (オプション)
```

**影響範囲**:
- `crates/core/src/providers/anthropic.rs`: リクエストボディに `thinking` 追加、ストリーミングイベントで `thinking` content block パース
- `crates/core/src/agent/types.rs:4-11`: `ContentBlock::Thinking` バリアント追加
- `crates/core/src/lib.rs`: thinking 有効化の設定 (環境変数 `THINKING_BUDGET` or config.toml)
- `agent/context.rs`: thinking 有効時は `max_tokens` の扱いが変わる (thinking_budget + max_output ≤ context limit)
- `crates/tui/src/main.rs`: thinking 表示 (AgentProgress イベントとして通知)

**補足**:
- ストリーミングでは通常の `text_delta` と同様に thinking 用 delta を逐次受け取り、`ContentBlock::Thinking` または `AgentProgress::ThinkingText` に変換してUIへ流す
- コンテキスト予算では「通常出力トークン + thinking budget」を合算して見積もる必要があり、`crates/core/src/agent/context.rs:45-64` の予算解決ロジックと `crates/core/src/agent/context.rs:245-255` のトークン推定ロジックの双方に影響する

**リスク**: 中。
- thinking 有効時はモデルの `max_tokens` セマンティクスが変わり、コンテキスト予算計算に影響
- thinking は一部モデルのみ対応 (Claude Sonnet 4+)。非対応モデルでの graceful degradation が必要
- thinking content block のストリーミングは通常の text delta と同じ仕組みだが、別のイベントタイプ

---

### 10. SOCIUM.md メモリ

**現状**: プロジェクト別の指示ファイルなし。システムプロンプトは固定 (`crates/core/src/constants.rs:27`) で、`AgentMessageRequest::new` も `DEFAULT_SYSTEM_PROMPT` を直接埋め込む (`crates/core/src/agent/types.rs:15-29`)。
**目標**: ワークスペースの `SOCIUM.md` ファイルをシステムプロンプトに注入。

```rust
// crates/core/src/agent/system_prompt.rs に追加 (Task 4 と統合)

const MAX_MEMORY_FILE_BYTES: usize = 50_000; // 50KB上限

/// SOCIUM.md ファイルの読み込み優先順位:
/// 1. $WORKSPACE/SOCIUM.md (プロジェクトルート)
/// 2. $WORKSPACE/.socium/SOCIUM.md (隠しディレクトリ版)
/// 3. ~/.config/socium/SOCIUM.md (グローバル)
fn load_memory_files(workspace_root: &Path) -> Vec<(String, String)> {
    let candidates = [
        (workspace_root.join("SOCIUM.md"), "project"),
        (workspace_root.join(".socium").join("SOCIUM.md"), "project (.socium/)"),
        (dirs::config_dir()
            .map(|d| d.join("socium").join("SOCIUM.md"))
            .unwrap_or_default(), "global"),
    ];

    let mut results = Vec::new();
    for (path, label) in &candidates {
        if !path.exists() { continue; }
        match std::fs::read_to_string(path) {
            Ok(content) if content.len() <= MAX_MEMORY_FILE_BYTES => {
                results.push((label.to_string(), content));
            }
            Ok(content) => {
                // トランケーション: 先頭 MAX_MEMORY_FILE_BYTES バイトまで
                let truncated = &content[..content.floor_char_boundary(MAX_MEMORY_FILE_BYTES)];
                results.push((label.to_string(), format!(
                    "{truncated}\n\n(truncated: SOCIUM.md exceeds {MAX_MEMORY_FILE_BYTES} bytes)"
                )));
            }
            Err(_) => continue,
        }
    }
    results
}

pub(crate) fn build_system_prompt(workspace_root: &Path) -> String {
    let mut parts = vec![DEFAULT_SYSTEM_PROMPT.to_string()];

    // ... 環境情報 (Task 4) ...

    // SOCIUM.md
    let memory_files = load_memory_files(workspace_root);
    for (label, content) in &memory_files {
        parts.push(format!("# Project instructions ({label})\n\n{content}"));
    }

    parts.join("\n\n")
}
```

**影響範囲**:
- Task 4 (システムプロンプト強化) と統合して実装
- `agent/system_prompt.rs` にファイル読み込みロジック追加
- `crates/core/src/agent/types.rs:15-29`: 固定 `DEFAULT_SYSTEM_PROMPT` 参照を、動的に構築した system prompt を受け取る形に変更
- `crates/core/src/agent/context.rs:245-255`: SOCIUM.md を含む system prompt 分をトークン推定に反映
- `crates/core/Cargo.toml:6`: `dirs` クレート追加 (グローバル設定ディレクトリ取得、オプション)

**リスク**: 低。
- ファイルが大きすぎる場合のトランケーション処理が必要 → 50KB上限
- コンテキスト予算への影響: SOCIUM.md の内容もトークン推定に含める必要あり (`crates/core/src/agent/context.rs:245-255`)
- 悪意あるファイル内容のリスクは低い（ユーザー自身が作成するため）

---

## 低優先度 (11-15) — 長期

### 11. MCPクライアント

**現状**: 外部ツールサーバーとの連携なし。ツールは `crates/core/src/tools/mod.rs:34-309` にハードコード。
**目標**: [Model Context Protocol](https://spec.modelcontextprotocol.io/) クライアントを実装し、外部ツールサーバーに接続。

```rust
// crates/core/src/mcp/mod.rs (新規)
pub(crate) struct McpClient {
    process: tokio::process::Child,  // stdio transport
    pending_requests: HashMap<u64, oneshot::Sender<Value>>,
}

impl McpClient {
    pub async fn connect(command: &str, args: &[String]) -> Result<Self, ...> { /* ... */ }
    pub async fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, ...> { /* ... */ }
    pub async fn call_tool(&mut self, name: &str, input: &Value) -> Result<String, ...> { /* ... */ }
}

// .socium/config.toml での設定
// [mcp.servers.filesystem]
// command = "npx"
// args = ["-y", "@anthropic/mcp-filesystem"]
```

**影響範囲**:
- 新規モジュール: `crates/core/src/mcp/`
- `tools/mod.rs`: MCP ツールを動的に追加する仕組み
- `tools/dispatch.rs`: MCP ツールへのディスパッチ
- `Cargo.toml` (core): `tokio` (process), JSON-RPC メッセージング
- `config.rs` (Task 8): MCP サーバー設定

**リスク**: 高。MCP プロトコル実装は非自明。stdio transport, JSON-RPC, capabilities negotiation, ツール定義の動的登録など。

---

### 12. セッション管理

**現状**: 単一会話のみ。`.socium/conversation.json` に上書き保存 (`crates/core/src/lib.rs:260-268`)。
**目標**: 複数セッションの切替、履歴ブラウザ、セッションID管理。

```rust
// crates/core/src/session.rs (新規)
pub struct SessionManager {
    sessions_dir: PathBuf,  // .socium/sessions/
}

impl SessionManager {
    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>, ...> { /* ... */ }
    pub fn create_session(&self, name: Option<&str>) -> Result<SessionId, ...> { /* ... */ }
    pub fn load_session(&self, id: &SessionId) -> Result<(Vec<Message>, ConversationSummaryState), ...> { /* ... */ }
    pub fn delete_session(&self, id: &SessionId) -> Result<(), ...> { /* ... */ }
}

pub struct SessionInfo {
    pub id: SessionId,
    pub name: Option<String>,
    pub created_at: SystemTime,
    pub message_count: usize,
}
```

**影響範囲**:
- 新規: `crates/core/src/session.rs`
- `crates/core/src/lib.rs`: `AgentService` の初期化パスをセッション対応に
- `crates/tui/src/main.rs`: セッション選択UI (起動時またはコマンド)
- `.socium/sessions/<id>/conversation.json` + `summaries.json`

**リスク**: 中。セッション切替時の state 整合性に注意。

---

### 13. LLMベース要約

**現状**: 決定論的要約 (`crates/core/src/agent/context.rs:279-313`) — メッセージを `- {Role}: {テキスト先頭}` に機械的に圧縮。
**目標**: コンテキスト圧縮時にLLMを呼び出してより高品質な要約を生成。

```rust
// crates/core/src/agent/context.rs に追加
async fn summarize_with_llm(
    client: &reqwest::Client,
    config: &ProviderConfig,
    model: &str,
    messages_to_summarize: &[Message],
) -> Result<String, Box<dyn std::error::Error>> {
    let summary_prompt = format!(
        "以下の会話を簡潔に要約してください。重要な決定事項、ファイル変更、未解決の問題を保持してください:\n\n{}",
        format_messages_for_summary(messages_to_summarize)
    );
    // 軽量モデル (haiku) で要約を生成
    let response = providers::send_message(/* haiku model, summary_prompt */).await?;
    Ok(response.text())
}
```

**影響範囲**:
- `crates/core/src/agent/context.rs`: `summarize_messages` を async 化、LLM 呼出追加
- `crates/core/src/agent/turn.rs` / `crates/core/src/lib.rs`: context 準備を async に
- API コスト増: 要約ごとにAPIコール

**リスク**: 中。要約のAPI呼出コスト、要約品質のばらつき、要約失敗時のフォールバック。

---

### 14. 構造化diffレンダリング

**現状**: ファイル編集結果はプレーンテキストで表示。TUIのMarkdownパーサー (`crates/tui/src/main.rs:1111-1170`) はコードフェンスのみ対応。
**目標**: ファイル編集時のunified diff表示をTUIで色付きで視覚的に表示。

```rust
// crates/tui/src/diff_renderer.rs (新規)
pub fn render_diff(diff_text: &str, area: Rect, buf: &mut Buffer) {
    for line in diff_text.lines() {
        let style = match line.chars().next() {
            Some('+') => Style::default().fg(Color::Green),
            Some('-') => Style::default().fg(Color::Red),
            Some('@') => Style::default().fg(Color::Cyan),
            _ => Style::default().fg(TEXT),
        };
        // ...
    }
}
```

**影響範囲**:
- 新規: `crates/tui/src/diff_renderer.rs`
- `crates/tui/src/main.rs`: `parse_segments` にdiff検出ロジック追加
- `crates/core/src/tools/patch.rs` (Task 1 後に新規想定): diff プレビュー出力をマーキング

**リスク**: 低。UI変更のみ。

---

### 15. 並行ツール実行

**現状**: `run_tool_calls_with_progress` (`crates/core/src/tools/runtime.rs:20-41`) が `filter_map` で逐次実行。
**目標**: 複数 tool_use ブロックを `tokio` で並列実行。

```rust
// 現在 (crates/core/src/tools/runtime.rs:28-40, 逐次)
blocks.iter().filter_map(|block| match block {
    ContentBlock::ToolUse { .. } => Some(run_tool_call_with_progress(...)),
    _ => None,
}).collect()

// 改善後 (並行)
let handles: Vec<_> = blocks.iter().filter_map(|block| match block {
    ContentBlock::ToolUse { id, name, input } => {
        let id = id.clone();
        let name = name.clone();
        let input = input.clone();
        let root = workspace_root.to_path_buf();
        Some(tokio::task::spawn_blocking(move || {
            run_tool_call(&root, &id, &name, &input)
        }))
    }
    _ => None,
}).collect();

let mut results = Vec::new();
for handle in handles {
    results.push(handle.await??);
}
```

**影響範囲**:
- `crates/core/src/tools/dispatch.rs` (Task 1 後に新規想定): `run_tool_calls_with_progress` を async に変更
- `crates/core/src/agent/turn.rs:59-60`: ツール実行呼出を `.await` に
- progress コールバック (`&mut F`): `Arc<Mutex<F>>` または `mpsc::channel` でラップ
- `crates/core/Cargo.toml:6`: `tokio` 依存

**リスク**: 高。
- LLMが同一ターンで依存関係のあるツールを呼ぶことがある（例: ファイル作成 → そのファイルの読み取り）。並行実行すると結果が不定になる
- progress コールバック (`&mut F`) のマルチスレッド共有が必要
- ツール間の副作用順序の保証が失われる
- 実用上の高速化効果は限定的（1ターンのツール数は通常1-3個、I/O待ちは短い）

**代替案**: `rayon` によるCPU並列化。ただしI/O主体のツールには効果が薄い。
**判断**: リスクに対して得られる効果が限定的。他の改善が安定してから実験的に取り組む。

---

## 実装順序の推奨

```
Phase 1 (基盤整備):
  Task 1 + Task 2  (ツール分割 + 型付き入力)  ※同時に実装

Phase 2 (API品質 + コスト):
  Task 3 (リトライ & エラー分類)
  Task 7 (コスト追跡)

Phase 3 (プロンプト + メモリ):
  Task 4 + Task 10  (システムプロンプト強化 + SOCIUM.md)  ※同時に実装

Phase 4 (UX):
  Task 5 (中断サポート — UI の "esc to cancel" 表示との整合)
  Task 6 (スラッシュコマンド — /compact, /model は core API 追加を含む)
  Task 8 (設定ファイル)

Phase 5 (高度機能):
  Task 9 (Extended Thinking)

Phase 6 (長期):
  Task 11-15 (MCP, セッション, LLM要約, diff表示, 並行ツール実行)
```

**変更点 (元プランからの調整)**:
- Task 5 (並行ツール実行) → 低優先度 Task 15 に降格。リスクが高く効果が限定的
- Task 15 (中断サポート) → 高優先度 Task 5 に昇格。UIの「esc to cancel」表示との乖離を解消
- Task 6 (スラッシュコマンド) の影響範囲を修正: TUIだけでなく core 側 API 追加が必要

---

## 検証方法

各タスク完了後の検証:
- `cargo check` — コンパイル確認
- `cargo test` — 既存テスト全通過 (lib.rs 内の約1,780行のテスト)
- `cargo clippy` — lint チェック
- 手動: TUIを起動して基本的なエージェント操作を確認

---

## KAIROS 調査からの示唆

`claude/` の追加調査では `CAIROS` は確認できず、代わりに `KAIROS` が assistant mode を中心とした大きな機能系統として統合されていた。詳細は `docs/analysis-kairos-in-claude.md` を参照。

この調査結果から、現在の Task 1-15 の優先順は概ね妥当と判断できる。

- **Task 5 (中断サポート)** を高優先度に置く判断は妥当
  - KAIROS 的な assistant mode / background execution / session continuity を考える場合も、まず UI 表示と実際の中断挙動が一致している必要がある
- **Task 4 + Task 10 (システムプロンプト強化 + `SOCIUM.md`)** は assistant-like mode の基盤として重要
  - KAIROS では brief / assistant addendum / memory 的な追加文脈が広く効いており、socium でも prompt layer を先に強化すべき
- **Task 12 (セッション管理)** は将来の session continuity の前提になる
  - KAIROS の bridge / continue / session-id 系の挙動に近づくには、単一会話モデルからの脱却が必要
- **Task 6 + Task 8 (スラッシュコマンド + 設定ファイル)** は mode 的な機能追加の前提になる
  - assistant mode や notifications を入れる前に、設定と操作面の入口を整える方が安全

### post-v0.3 候補

KAIROS 調査から見えてきたが、現在の Task 1-15 にはまだ入れていない候補:

- assistant mode 相当の umbrella mode
  - セッション継続
  - バックグラウンド実行
  - team / sub-agent 文脈の共有
- channels / notifications
  - TUI 外での通知と応答
  - 添付や承認の relay
- durable loop / triggers
  - cron 的な定期タスク
  - リモート trigger

これらは Task 1-15 完了後に、別フェーズとして切り出すのが適切。
