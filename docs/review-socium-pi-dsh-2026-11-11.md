# socium設計・実装レビュー — pi / DeepSeek Harness比較
調査日: 2026-11-11。対象: socium `de374125a5ff00cc54b8f94da020250b3ab6b46c` の作業ツリー。実装変更は行っていない。

**結論: sociumには、保守しやすい小さなコーディングエージェントに育てる土台がある。一方、現状を「承認付きで安全に使え、長時間の作業を継続できるエージェント」と評価することはできない。**
問題の中心はツール数やプロンプトの長さではなく、実行権限、プロセスの生存期間、APIのメッセージ契約、失敗後の記録の一貫性である。日常利用の前提になるこれらを先に直すべきだ。

piはDeepSeek製ではない。旧badlogic/pi-monoから転送される、現在のearendil-works/piを確認した。DeepSeek製はdeepseek-ai/deepseek-harness（dsh）。最新ブランチを無条件に同一視せず、次のコミットに固定して読んだ。

| 対象 | 固定したコミット | 主に確認した範囲 |
|---|---|---|
| socium | `de374125a5ff` | core・TUIの主要実装、テスト、CI、Nix、既存設計文書 |
| [pi](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/README.md) | `682c7b93ce3e` | agent-loop、session-manager、bash、edit、compaction、system-prompt |
| [dsh](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/README.md) | `fcc0ad462172` | architecture、agent-loop、assistant-stream、user-approval、permission-presets、compaction-basic、agent-instructions、SAFETY |

**検証範囲と限界。**
sociumの既存テストはcore 95件、TUI 13件、計108件が成功した。format checkとClippyの全ターゲット・全feature・警告エラー化も成功。追加した15件の観察用テストでは、後述の不具合の成立条件を確認した。フィルタに既存のpreviewテスト1件も一致するため、再現ログの表示は16 passedになる。追加テストは「バグが存在すると成功する」検証であり、修正後の合格条件ではない。

実APIへの有料呼び出し、実モデルによるタスク性能比較、pi/dshのインストール・全テスト実行、Nix build、macOS/Windows動作確認は行っていない。したがって「sociumの正答率がpiより低い」「Rustだから高速」といった性能上の断定はしていない。API形式の指摘は生成されるリクエストと公式仕様の照合であり、実サーバーで400応答を観測したという意味ではない。既存の未追跡資料はそのまま残した。

Cargoの標準キャッシュが読み取り専用だったため、依存キャッシュを一時領域にコピーしてオフラインで検証した。既存のコードを一時コピーして再現テストを注入し、削除・書き込み・Git操作はそのテストが作る一時ファイル／一時リポジトリだけで行った。

[既存テスト結果](/home/chansuke/code/myprojects/socium/docs/review-evidence/2026-11-11/baseline-tests.log)、[Clippy結果](/home/chansuke/code/myprojects/socium/docs/review-evidence/2026-11-11/clippy.log)、[再現結果](/home/chansuke/code/myprojects/socium/docs/review-evidence/2026-11-11/reproductions.log)、[再現用ランナー](/home/chansuke/code/myprojects/socium/docs/review-evidence/2026-11-11/run_repros.py)を添付する。

**まず維持すべき良い点。**

| 良い点 | 根拠と価値 | 留保 |
|---|---|---|
| coreとTUIの分離 | UIを触らずツール・provider・contextを試験できる。[AgentService](/home/chansuke/code/myprojects/socium/crates/core/src/lib.rs:100) | 公開SDKとしての拡張口はまだ不足 |
| 小さく追える構造 | Rustファイル42個、テスト・example込み9,965行。巨大な制御フレームワークが不要な規模 | 行数自体は品質や速度の証拠ではない |
| ツール定義と実行の同居 | [Tool traitと登録表](/home/chansuke/code/myprojects/socium/crates/core/src/tools/mod.rs:59)により名前・schema・実装の対応を追いやすい | 固定登録とprovider固有schemaは分離する余地がある |
| 構造化argv | シェル文字列の暗黙評価を避ける設計は妥当 | argv方式でも権限境界にはならない |
| ファイルツールの境界検査 | 既存パスのcanonicalize、絶対パス拒否、symlinkテストがある | 新規パスの検査に実際の穴がある |
| 編集時の曖昧性拒否 | str_replaceは0件・複数一致を拒否する。[実装](/home/chansuke/code/myprojects/socium/crates/core/src/tools/text_editor.rs:94) | apply_patchには同等の曖昧性拒否がない |
| UIの応答性への配慮 | blockingツールを専用OSスレッドに逃がす。[実装](/home/chansuke/code/myprojects/socium/crates/tui/src/agent_task.rs:47) | 中断・終了・panic時の所有権管理が未完成 |
| ストリーミングの分離 | SSE parserとmessage builderを分け、分割chunkやusageをテストする | 終端・stop reasonを検証していない |
| 要約を生履歴と分離 | summaryとraw messagesを分けた方向は正しい | 保存の原子性とrequestへの投影が不十分 |
| CIと再現性 | fmt、Clippy、tests、Nix buildをCI化し、Actionの参照をSHA固定 | Linux中心。意味論上の欠陥はこれらのチェックでは拾えていない |

全面書き直しは不要。とくにTool trait、ScriptedProvider、AgentProgressは改善の足場になる。traitが存在しない、テストがない、ツールが巨大な一ファイルにまとまっている、という評価は現在の実装には当てはまらない。

**最優先の指摘。** P1は通常利用の信頼性・データ保護を損なうため先に直すもの、P2は基盤修正後の品質・保守性改善を表す。ここでは、仮定を置かず全利用者に即座の全面破壊が起きると示したわけではないため、P0を乱用しない。

| ID | 優先度 | 問題 | 根拠の強さ |
|---|---|---|---|
| F1 | P1 | モデルが自分で承認を付与できる | 再現 |
| F2 | P1 | 新規パスの `..` でworkspace外へ作成できる | 再現 |
| F3 | P1 | read-only分類とworkspace制限が実行を拘束しない | 再現 |
| F4 | P1 | pipe詰まり、子孫プロセス、signal終了の扱い | 3条件で再現 |
| F5 | P1 | 途中API失敗で実行済み編集の会話記録を失う | 再現 |
| F6 | P1 | 要約がAPI契約を破り、tool-resultを孤立させる | 生成データ再現＋公式仕様 |
| F7 | P1 | stream未完了・出力上限到達を正常完了扱い | builderで再現 |
| F8 | P1 | patch部分適用と、検証しないdry-run | 再現 |
| F9 | P1 | git_commitが無関係な変更までstageする | 再現 |
| F10 | P1 | Escの中断表示と実動作が不一致 | コード追跡 |
| F11 | P2 | 保存が非原子的、複数プロセス・履歴切詰めに弱い | コード追跡 |
| F12 | P2 | provider中立化・context予算・prompt・検索・UXの不足 | コード追跡 |

**F1 — 承認はユーザーの意思ではなく、モデルが出した文字列になっている。**

[run_command](/home/chansuke/code/myprojects/socium/crates/core/src/tools/run_command.rs:140)はJSONの `approval` を読み、必要水準以上なら実行する。tool schemaはこのフィールドをモデルに公開しており、不足時のエラーも「その値を設定して再実行せよ」と伝える。[git_commit](/home/chansuke/code/myprojects/socium/crates/core/src/tools/git.rs:327)の `confirmed: true` も同じ構造だ。承認イベントは[TUI](/home/chansuke/code/myprojects/socium/crates/tui/src/progress.rs:13)で表示されるだけで、ユーザー応答を待つ双方向経路がない。

一時ファイルに対して `{"command":["rm","disposable.txt"],"approval":"destructive"}` を与えると、人間の応答を一度も入力せず削除できた。これは「モデルがユーザーの承認を忠実に転記するはず」という期待であって、承認機構の保証ではない。

修正はapprovalフィールドの説明追加では足りない。モデルが出すのは実行要求までにし、ランタイムが `call_id`、正規化argv、cwd、権限範囲に結び付いた承認要求を生成する。TUIなど信頼できる入力経路からの応答を受け、ランタイム内のgrantで実行を許可する。拒否・期限切れ・切断は実行しない。既に許可された操作を毎回聞き直す必要はなく、事前ポリシーやユーザーの継続的な許可をruntime側で扱えばよい。

dshの参考点は[UserApproval.request](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/interaction/user-approval/src/index.ts#L225)。独立したサービスが要求IDを生成し、asked/decidedを記録し、answerer不在をfail closedにする。「同じbooleanを使う」ことではなく、承認情報をモデル入力と別の経路で所有する点を採用する。

**F2 — 新規ファイルのパス境界に、競合なしで成立する脱出がある。**

[resolve_workspace_path_for_create](/home/chansuke/code/myprojects/socium/crates/core/src/tools/workspace.rs:69)は、対象が未作成だと存在する祖先まで遡る。祖先がworkspace内かを検証した後、未正規化の `joined` を返す。

workspace直下の `missing` が存在しないとき、`missing/../../escaped.txt` をAdd Fileに与えると、後続の `create_dir_all` が `missing` を作り、`..` が実際に解決され、workspaceの親へ書き込めた。symlink差替えや別プロセスとのraceを必要としない。text editorのcreate系も同じresolverを共有している。

修正は、パスの各componentを正規化・検証し、未作成suffixを含めworkspace外への脱出を拒否すること。既存部分のsymlinkを解決した結果と未作成部分の合成後の境界を検査する。非信頼プロセスとの並行実行も対象にするなら、descriptor基準のアクセスやOS sandboxを組み合わせる。単に `canonicalize` を一度増やすだけでは未作成パスとTOCTOUを両方扱えない。

**F3 — コマンド名の分類は、read-onlyやworkspace内実行の保証になっていない。**

[is_safe_read_only_command](/home/chansuke/code/myprojects/socium/crates/core/src/tools/run_command.rs:349)は `sed`、`find`、`env`、`sort` などを引数を見ずに許可する。`sed -i` で書き換えられ、`env` 経由なら任意のプログラムを起動できる。実行ファイルはbasenameだけで分類するため、別の実体が同名でも同じ扱いになる。

実際に `sed -i s/before/after/ file.txt` が「safe-read-only」として承認なしで書き換えた。また、一時workspace外の非機密fixtureを絶対パス指定した `cat` で読めた。`current_dir` とcwd検証は作業開始位置を決めるだけで、ファイル・ネットワーク・環境変数へのアクセスを制限しない。子プロセスには親の環境が引き継がれるので、provider用credentialとtool実行環境も分離して考えるべきだ。

分類はUXや承認判断の補助に限定し、実効的なread/write/network権限はexecutor側で強制する。制約を実装しない段階では、ユーザー権限で任意実行するモードだと明示し、「workspace-write」の名称だけで安全性を示唆しない。

piも[標準ではpermission制限を持たないと明記](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/README.md#L268)している。この透明性は良い。piが無制限だからsociumの疑似承認で十分、という結論にはならない。

**F4 — コマンド実行は、普通のビルドログでも止まり得る。**

[execute_command](/home/chansuke/code/myprojects/socium/crates/core/src/tools/run_command.rs:218)はstdout/stderrをpipeにしたまま `try_wait` で終了を待ち、最後にだけ `wait_with_output` で読む。出力がpipe容量を超えると、子はwriteを待ち、親は子の終了を待つ。

1 MiBの一時ファイルを `cat` するだけで、正常なコマンドが150msのtimeoutになった。表示用の20,000文字上限は取得後に適用されるので、この詰まりも、取得時のメモリ増加も防がない。

さらに次の2条件を確認した。
- `sh -c "sleep 0.4 &"` はtimeout 50msでも約400ms待ち、Timed out: falseを返す。親shell終了後も子孫がpipeを保持し、`wait_with_output` が待つため。
- `sh -c "kill -TERM $$"` はExit code: terminatedなのに、`exit_code.unwrap_or_default() != 0` によって成功扱いになる。

修正はspawn直後から両streamを同時に排水し、保持量に上限を設け、全量は必要に応じて一時ファイルへ退避すること。タイムアウトとキャンセルはprocess group／Windows Job Objectなどで子孫を含めて扱い、`status.success()` とsignalを区別する。終了・drain・cleanupすべてに期限と状態を持たせる。

[pi bash](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/coding-agent/src/core/tools/bash.ts#L112)はonDataによる逐次回収、process tree停止、signalの終了コード化、出力退避を実装する。ここは少数ツールでも実用性が高くなる具体的な差である。

**F5 — 「失敗したがファイルは変わった」を記録できない。**

[submit_with_progress](/home/chansuke/code/myprojects/socium/crates/core/src/lib.rs:275)はcontextのローカルコピーでturnを実行し、成功した場合だけ差分を `self.messages` へ戻して保存する。turn内のツールには実際の副作用がある。

「1回目の応答でapply_patch成功 → 2回目のprovider呼び出しが失敗」のScriptedProviderを作ると、ファイルは作成されたが、AgentServiceにはユーザー入力1件しか残らず、conversation.jsonも作られなかった。20 tool roundsの上限でエラーになる場合にも同種の経路がある。再起動後の監査だけでなく、同じプロセスで次に入力したときのモデル認識も実ファイルとずれる。

ツールを実行する前にintentとcall IDを記録し、結果を逐次確定する。turn全体の成功を保存条件にしない。履歴の意味を `Attempt / ToolStarted / ToolFinished / TurnFailed` などの事実に分け、再開時は途中状態を扱えるようにする。副作用とログ保存を完全に一つのtransactionにできない操作については、「実行されたか不明」を表現し、非冪等な操作を自動再実行しない。

piの[session-manager](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/coding-agent/src/core/session-manager.ts#L1173)はユーザーメッセージから保存を開始し、その後entryごとにappendする。dshの[resume処理](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/core/agent-loop/src/index.ts)はwrite ownershipを取得し、中断したturnの欠落tool結果や終端を補修する。どちらも「最後に全部保存」とは故障時の意味が違う。

**F6 — 長い会話は予算以前に、メッセージ構造で壊れる。**

[prepare_context_with_budget](/home/chansuke/code/myprojects/socium/crates/core/src/agent/context.rs:128)はまず最新24メッセージだけを保持する候補を作る。全履歴が予算内ならそのまま送る、という初期候補がない。25個の短いmessageで、推定入力1万token未満でも1件を要約することを確認した。24はユーザーの24ターンではなく、assistant/tool-resultを含む内部message数である。

その要約を[summary_message](/home/chansuke/code/myprojects/socium/crates/core/src/agent/context.rs:264)が `messages[0].role = "system"` として送る。Anthropic adapterはこれをトップレベルsystemへ変換しない。現在の[公式仕様](https://platform.claude.com/docs/en/build-with-claude/mid-conversation-system-messages#limitations)でも、本文を持つsystem messageをmessages配列の先頭に置けない。新しいモデルにmid-conversation system対応があることと、この配列が有効であることは別問題だ。

加えて、emergency trimmingは任意message数で切り、先頭に孤立したtool_resultを残せることを再現した。LLM compaction側にある `retention_boundary` の配慮が、通常のcontext準備経路には使われていない。

修正は以下を一つのContextAssemblerの責務として統一する。
- 最初に全履歴＋system＋tools＋出力予約を評価し、必要なときだけ圧縮する。
- tool call/resultやthinkingを含むproviderの会話単位を壊さない境界を選ぶ。
- summaryを内部表現として保存し、provider adapterが対応する合法な会話形式へ投影する。
- summary内の引用・tool出力を、無条件に信頼されたsystem instructionへ昇格させない。
- 予算に収まらないときは「収まったことにして送る」のではなく、追加の縮約か明示的なエラーにする。

これは一つのroleを書き換えるだけで終わる修正ではない。summary生成、保持境界、provider形式の契約をまとめて試験する必要がある。

**F7 — streamの完了と回答の完了を区別していない。**

[StreamingMessageBuilder](/home/chansuke/code/myprojects/socium/crates/core/src/providers/anthropic/stream.rs:130)は `message_stop` と `content_block_stop` を無視し、`message_delta` のstop_reasonを持たない。`finish()` は完了イベントを受けたか確認しない。

message_stopなしでもtool-use blockを正常な結果として返すこと、`max_tokens` が来ても通常のtext結果になることをbuilderテストで確認した。ネットワークのchunk読取りがエラーになった場合は別途Errになるが、HTTP bodyが正常なEOFとして終わった不完全streamは区別できない。完成したJSONだけあっても、モデル応答全体が正常に終了したとは限らない。

`StopReason { EndTurn, ToolUse, MaxTokens, Refusal, Cancelled, Error, ... }` をprovider responseに載せる。受信完了、block完了、tool引数の検証を満たす前にツールを実行しない。max_tokensは、継続可能か、部分結果を残して止めるかを別扱いにする。retryにはattempt IDとUI上の部分出力の扱いも必要。

[pi agent-loop](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/agent/src/agent-loop.ts#L253)はerror/abortedを通常完了と区別し、length終了のtool呼び出しを実行せず失敗結果にする。ここは明確に取り込むべき契約である。

**F8 — patchのdry-runと本適用の意味が一致しない。**

[apply_patch](/home/chansuke/code/myprojects/socium/crates/core/src/tools/apply_patch.rs:88)はactionsを順に書き込む。後のファイルで失敗しても前のファイルは戻らない。「Add first.txt → Delete nonexistent.txt」でErrとfirst.txtの残存を再現した。呼出元に部分適用済みの構造化結果も返らない。

[preview_patch_actions](/home/chansuke/code/myprojects/socium/crates/core/src/tools/apply_patch.rs:362)はAdd/Updateの実在・境界・hunk一致・移動先競合を本適用と同じ方法で検証しない。存在しないファイルのUpdateでもdry_runは成功、本適用は失敗した。diff表示も行の `contains` 判定なので、同じ行の順序変更や重複の増減を正確には表現できない。

最低限、全actionsを読み取り・検証してMutationPlanを構築し、同じplanからpreviewとapplyを行う。適用直前には元内容のhash等を照合する。複数ファイルの完全なatomicityを約束できないなら、書込み済み操作と復旧情報を返す。apply_patchはtext_editorのedit historyに参加していないため、現状のundoは全編集を戻せる機能でもない。

追加の問題として、hunkは最初の一致を採用し、重複候補の拒否がない。空ファイルへのinsertion-only hunkは未対応。厳格なstr_replaceとpatchで編集契約が違う。piの[edit](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/coding-agent/src/core/tools/edit.ts#L154)は同一ファイルのmutation queueと全置換の事前照合、diff先行生成を持つが、これもプロジェクト全体のtransaction保証ではない。参考にする保証の範囲を限定すべきだ。

**F9 — commitはユーザーが選んだ変更範囲を尊重できない。**

[git_commit](/home/chansuke/code/myprojects/socium/crates/core/src/tools/git.rs:351)は必ず `git add -A` を呼ぶ。既にrequested.txtだけstageされた一時repoで、無関係なunstagedのunrelated.txtまでcommitに入ることを確認した。一般的な「この修正をcommitして」という依頼と、全未追跡・全変更をcommitする操作は一致しない。

パスを明示する入力、staged-onlyの既定、stage差分の提示、事前indexを壊さない失敗処理が必要。`confirmed` の自己申告問題はF1と同時に直す。編集後にdirty警告を付けるだけでは、既存のユーザー変更の保護にはならない。

**F10 — 中断できないのに、中断できるUIを出している。**

[thinking表示](/home/chansuke/code/myprojects/socium/crates/tui/src/view/text_layout.rs:105)には「esc to cancel」とある。実際の[Esc処理](/home/chansuke/code/myprojects/socium/crates/tui/src/app.rs:312)は入力モードをNormalに変えるだけで、HTTP・ツール・子プロセスへcancelを送らない。進行中は通常の入力も拒否するため、ユーザーによる方針修正や承認応答の入力もできない。

専用threadのhandleやcancel tokenはPendingTaskが持っていない。さらにoneshotの `try_recv` がClosedになった場合を[poll_pending_task](/home/chansuke/code/myprojects/socium/crates/tui/src/agent_task.rs:136)が処理しないため、worker panic等では待機状態から回復できない経路がある。これは静的追跡の指摘で、実UIでpanicを注入した再現ではない。

最初に表示を実態に合わせ、次にAgentControlを導入する。CancelとApprovalResponseはUI→core、ProgressとResultはcore→UIとし、HTTP、tool、process tree、保存に一貫した中断理由を渡す。steeringキューはその後でよい。終了時も実行中processをどう収束させるか決める。

**F11 — 保存は正常終了時以外の一貫性が弱い。**

[save_json_file](/home/chansuke/code/myprojects/socium/crates/core/src/agent/state.rs:45)はJSON全体を `fs::write` する。truncate後の中断に弱く、conversationとsummaryは別ファイルへ順次書くためpairとしての原子性もない。load失敗時は空会話へ進む。複数sociumプロセスが同じcwdで動けば同じ保存先を上書きできる。

[from_env](/home/chansuke/code/myprojects/socium/crates/core/src/lib.rs:153)は2,000件超の履歴を先頭からdrainするが、summaryのcovered_messagesを同じ量だけ調整しない。`covered_messages <= messages.len()` なら古いsummary indexがそのまま通り、違う範囲を要約済みと扱い得る。切り詰め境界もtool pairを考慮しない。実行中メモリはこの上限で抑制されない。

当面はtemp-file＋rename、書込エラーの明示、単一writerロック、schema versionで改善できる。その後、session ID・event sequence・parent/compaction参照を持つappend-onlyログへ移す。JSONLであるだけで安全になるわけではなく、tail切断、flush方針、未知version、writer競合、部分実行の回復を定義する。piにも全entryを書き直す経路があるため、「piは常にatomicで完全」とは評価していない。

**F12 — provider・context・tool・UXには次の設計上の課題がある。**

| 領域 | 現状 | 推奨 |
|---|---|---|
| Provider | traitは既にあるがrequest、Role、ToolDefinitionがAnthropic寄り。Codex/OpencodeはNotImplemented | 共通message/tool/usage/stop reasonとprovider変換を分離。最初に2つの実providerで契約を検証 |
| 能力情報 | supports_toolsのみ | model ID、context/output limit、streaming、reasoning、image、schema制約をconfig/capabilitiesで表現 |
| 設定 | MODEL既定とcompaction modelがClaude固定、API URL固定、env読取が分散 | 起動時に検証済み設定へ集約し、endpointとmodel registryを導入 |
| Context予算 | 文字数÷3.5/4.0/4.2の推定。usageは最後のcall分のみ | 実測＋その後に増えた入力の推定。各model requestの直前に再評価 |
| turn内増加 | prepare_contextはsubmit時だけ。tool round中の結果増加を再予算化しない | 大きなtool出力をartifact化し、roundごとに予算・compactを判断 |
| 要約品質 | 600文字/メッセージの先頭抜粋、決定論的要約の追記を先頭4,000字で切る | 最新修正・未解決事項・検証結果を保持する構造化summaryと復元評価 |
| API障害 | timeoutはあるがretry、backoff、Retry-After、型付きstatus分類がない | 429/一時5xx等を限定retry。副作用後の自動再実行と切り離す |
| 使用量・コスト | 最新usageのみ保持、累積・compaction分の観測なし。cache_control送信なし | 全attemptの使用量、latency、cache hitを記録。stable prefixを測定して最適化 |
| Prompt | 固定promptは簡潔さとpath:line引用が中心。project instructions/skillsのloaderなし | 小さい作業規約＋AGENTS.mdのscope/優先順位/サイズ上限。ツール出力との信頼区別 |
| ツールの出力 | 多くが文字列のみ。truncationやexitを後段が再解釈する | structured metadata＋モデル向けtext＋大容量artifact参照 |
| 検索 | rg結果を全回収してから件数制限。fallbackはregexではなく部分文字列 | 時間・bytes・行長の上限、regex契約統一、rg JSON形式 |
| 検索の境界 | pattern前に `--` がなく、先頭hyphenがoptionとして解釈され得る。colon区切りparse | `-e pattern -- path` 等を使い、特殊ファイル名もfixture化 |
| 言語対応 | find_symbolはRustの宣言らしい行を探すheuristic | 名称・能力を正直に示す。必要な言語からtree-sitter/LSP等を検討 |
| Session | cwdごとに会話1本。再起動時tool-only履歴はtranscript_entriesが除外 | named/new/resumeと、永続ログからの会話・tool双方の投影 |
| UI | 10行header＋8行補助paneで小画面の会話領域が狭い。stdout先頭がstderr表示を押し出す | 実行結果・エラー末尾を優先。領域可変・折畳み・実model表示 |
| 文字幅 | 入力／折返しの多くがchars数を幅と扱う | 日本語全角・結合文字・絵文字の表示幅とカーソルを検証 |
| 配布 | Nixでgit/rgの実行時PATHを包んでいない。rgなしでは検索意味が変わる | 必須/任意依存を明示してpackageに同梱または起動時確認 |
| 品質基盤 | CIは良好だが、仕様違反を許すmockや正常系中心のテストが残る | wire contract・故障注入・再開・権限・プロセス管理を優先して追加 |

[provider契約](/home/chansuke/code/myprojects/socium/crates/core/src/providers/mod.rs:37)、[tool schema](/home/chansuke/code/myprojects/socium/crates/core/src/tools/mod.rs:33)、[token推定](/home/chansuke/code/myprojects/socium/crates/core/src/agent/context.rs:305)、[要約](/home/chansuke/code/myprojects/socium/crates/core/src/agent/compaction.rs:106)、[rg実行](/home/chansuke/code/myprojects/socium/crates/core/src/tools/ripgrep.rs:100)、[UI layout](/home/chansuke/code/myprojects/socium/crates/tui/src/view/mod.rs:19)、[Nix配布](/home/chansuke/code/myprojects/socium/flake.nix:22)を参照。

ここでの不足は全部を今すぐ実装する意味ではない。たとえば全toolをasync化したり、providerを大量に増やしたりする前に、既存の12ツールに共通するExecutorとSessionの保証を固める方が効果が高い。AGENTS.mdがないことは実装品質の欠陥というより製品能力の不足だが、実プロジェクトで一貫したコードを出すための投資対効果は高い。

**piから学べることと、その限界。**

piは最小の既定tool surfaceを保ちつつ、複雑さをsession、provider、実行、拡張境界へ移している。現在のmonorepoにはdurableやchordなどもあり、「piの全コードが小さい」と理解するのは正確ではない。[README](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/README.md)と[system prompt実装](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/coding-agent/src/core/system-prompt.ts#L60)を区別して読むべきである。

| 観点 | piで確認したもの | sociumへの判断 |
|---|---|---|
| 基本tool | read/bash/edit/writeが既定 | 12→4へ削れば賢くなるとは限らない。一般モデルで4〜6、弱いlocal modelで専用検索を加える等を評価する |
| Loop | steering/follow-up、abort、stop reason、イベント境界 | Cancelとfinish reasonを先に導入。自律継続の複雑さは後でよい |
| Session | ID/parentId付きentry、append、branch、version migration | まずsession IDと逐次記録。tree/forkは基盤が固まってから |
| Command | 逐次出力、process tree停止、出力退避 | F4の直接的な実装参考になる |
| Edit | mutation queue、unique/non-overlap検証、diff先行、BOM/改行配慮 | 元ファイルと編集planの契約を共通化する |
| Context | usage＋後続分推定、token保持予算、compactionの専用処理 | message件数の常時切捨てを置き換える |
| Prompt | toolに応じたguideline、project context、skills、cwdを組み立て | 巨大固定promptより、小さなsectionと入力契約を採用する |
| 接続形態 | CLIに加えprint/JSON/RPC/SDKを公開 | core分離を活かし、Web UIより先にheadless JSONL経路を検討 |

参考: [loop](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/agent/src/agent-loop.ts)、[session](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/coding-agent/src/core/session-manager.ts)、[bash](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/coding-agent/src/core/tools/bash.ts)、[edit](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/coding-agent/src/core/tools/edit.ts)、[compaction](https://github.com/earendil-works/pi/blob/682c7b93ce3e0a52b579c2b65b4c06bb392e7180/packages/coding-agent/src/core/compaction/compaction.ts)。

piにも限界がある。標準のhost実行に権限制限はなく、sandboxは別の運用・拡張に依存する。exact editやmutation queueは外部エディタを含む全競合をなくすものではない。sessionにrewrite経路があり、JSONL採用だけで全障害の安全性が保証されるわけでもない。モデル能力やextension品質に依存するため、minimalという思想そのものを性能の証拠にしてはいけない。

**dshから学べることと、その限界。**

dshの強みは「everything is a plugin」という標語だけではない。modelに見せるtool、policy、実際のfilesystem/shell、session log、UIを別の責任として設計し、同じ実行環境へ差し替えられることにある。[設計文書](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/docs/architecture.md)。

| 観点 | dshで確認したもの | sociumへの判断 |
|---|---|---|
| 承認 | request ID、asked/decided audit、answerer不在時の拒否、cancel | F1に必要な最小ApprovalServiceをRustで導入 |
| 実行環境 | shell/fs等のserviceとsandbox policyを分離 | toolごとの安全判定を増やすよりExecutorに集約 |
| 永続化 | session event logからmodel/UI状態を投影。中断turnの補修 | 実行済み操作と会話の不一致を減らすため採用 |
| 可視stream | transient chunkとdurable message/attemptを分離 | UIの部分表示と確定履歴を同じものにしない |
| Context | step境界のpressureとprovider overflow回復を区別 | submit時だけのcompactを見直す参考 |
| Project規約 | AGENTS/CLAUDEのchain、byte予算、nested file更新、出典付き履歴 | 当面はAGENTS.mdのみでよいがscopeと予算は最初から設計 |
| Profile | web/headless/sdk/acp等をbundleから構成 | 外部plugin ABIやprofile DSLの導入は時期尚早 |

参考: [approval](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/interaction/user-approval/src/index.ts)、[permission presets](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/interaction/permission-presets/src/index.ts)、[loop](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/core/agent-loop/src/index.ts)、[stream attempt](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/core/agent-loop/src/assistant-stream.ts)、[compaction](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/compaction/compaction-basic/src/index.ts)、[instructions](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/context/agent-instructions/README.md)。

dshはpluginの初期化・破棄順、serviceの有無、context scope、profile/bundleの合成を正しく管理する必要があり、追跡する制御経路も増える。permission-presetsがunconfined executor上の構成を拒否する検証を持つこと自体、この合成の難しさを示している。socium規模でCordis相当を自作すると、本来直すべき基盤の前にframework保守を抱えやすい。

また[SAFETY](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/SAFETY.md)は、developer preview、未security audit、production-readyと見なさないこと、sandboxを唯一の防御にしないことを明記している。高度な設計を安全性の認証と取り違えない。instruction loaderにもshell経由のディレクトリ移動ではnested discoveryされない等の制限が記載されている。dshを欠点のない到達点として扱わない方が、比較として公平である。

**3製品の位置づけ。**

| 軸 | socium | pi | dsh |
|---|---|---|---|
| 現在の構成 | Rustの小さいcore＋TUI | 最小既定toolと再利用可能なagent/provider/session群 | Cordisによる構成可能なservice/plugin群 |
| 強み | 実装を把握しやすい、静的な構造、手元で変更しやすい | 小さい入口に対して実行・context・sessionが厚い | 承認・実行環境・永続化・UIの境界が明確 |
| 主な負債／tradeoff | 権限と実行履歴の保証が見かけに追い付いていない | host権限依存、拡張に寄せる運用負担 | 合成・lifecycle・移行の複雑さ、previewの変化 |
| sociumが採るべき要素 | 既存のcore/TUI分離を維持 | process/context/sessionの堅牢化 | policy/executor/logの責任分離 |
| すぐには不要な要素 | ツール数の増量 | 大規模extension生態系の再現 | 全面plugin化、hot reload、複数UI一括導入 |

これはソースからの設計評価であり、同一タスクを実行した性能ランキングではない。

**既存の改善計画への批判。**

[improvement-plan.md](/home/chansuke/code/myprojects/socium/docs/improvement-plan.md:1)は旧 `tools/runtime.rs` や `providers/anthropic.rs` を前提とし、ツール分割とtrait化を重要な未解決作業として扱う箇所がある。しかし現在は既に分割・trait化されている。一方、[todo.md](/home/chansuke/code/myprojects/socium/docs/todo.md:1)は権限、context budgeting、dirty warning等を完了扱いにしている。

| 既存記述 | 今回の評価 |
|---|---|
| Shell with approvals: 完了 | ユーザー承認の実在を検証しないので、実効的な承認として未完了 |
| Keep workspace boundaries: 完了 | F2/F3により保証できていない |
| Model-aware budgeting: 完了 | モデル名による文字数係数＋一部usage実測。requestごとのbudget保証ではない |
| Dirty warnings before edits: 完了 | 多くは編集後にdirtyを検査・表示するだけ |
| Undo/edit history: 完了 | text_editor中心。apply_patchを含む統一undoではない |
| Input history navigation: 未完了 | 現在のapp/input_historyには既にUp/Down、Ctrl+P/N、保存がある |
| Core loop integration tests: 未完了 | ScriptedProviderとcompaction系serviceテストはある。ただし今回の故障条件が不足 |
| 「prompt → memory → execution」の順 | F1〜F10の実害を先に直す方が妥当 |

計画の価値は残るが、現在の実装を測る根拠にはそのまま使えない。実装ファイルが存在することをDoneの条件にせず、観測できる受け入れ基準に置き換える。既存文書の優先順位を今回は無断で書き換えず、このレビューを差分の根拠として残す。

**sociumに適した設計案。**

推奨するのは、現在の2-crate構成を保ち、coreの中で責任を明確にする設計である。次の要素を、既存のTool/Provider/AgentProgressから段階的に抽出する。

| 要素 | 所有するもの | 所有しないもの |
|---|---|---|
| AgentDriver | turn/step/attemptの遷移、終了理由、停止条件 | OS権限の判断、TUI描画 |
| ProviderAdapter | wire request/stream、model capabilities、usage/stop変換 | ファイル操作、ユーザー承認 |
| ToolRegistry | provider非依存schema、入力検証、result metadata | grantの発行 |
| Executor | filesystem/process、sandbox、stdout drain、cancel、artifact | モデルへの会話投影 |
| ApprovalService | 信頼された許可ポリシー、要求ID、応答、grant、audit | モデルが宣言した「承認済み」の信用 |
| SessionStore | ID/version/sequence、逐次事実、writer所有権、復旧 | UIでの見せ方 |
| ContextAssembler | raw logからの合法なmessage投影、budget、summary、instructions | 実行済み事実の削除 |
| AgentControl/Event | cancel、approval response、将来のsteeringと進捗 | 可変なAgentServiceをUIごとに分岐実装すること |

実行順は、ToolProposal → validate → policy/approval → 実行開始記録 → executor → 実行結果記録 → UI/modelへ投影、を基本とする。ユーザーの一度の許可を毎回聞き直す設計にはせず、scopeを明示してruntimeが再利用する。readonly並列化は、この順序と書込競合の規約を固めてから追加する。

ログに「意図」を書けばexactly-onceになるわけではない。外部commit・process・filesystemの副作用とローカルログの間にはクラッシュ窓がある。完了記録のない実行はUnknownとして扱い、確認なしの再実行を避ける設計が必要である。

Promptは小さく保ってよい。最低限、作業の目的、inspect→edit→verify、ユーザー変更の保護、実施していない検証を成功と述べないこと、project instructions、実行制約と拒否後の扱いを明確にする。これらの文章はruntime制約の補助であり、F1/F2をpromptで修正しようとしてはいけない。

**実装順と完了条件。**

| 順番 | 作業 | 完了を判断する条件 |
|---|---|---|
| 1 | F1/F2/F3: permissionとpath境界 | モデルの入力だけではgrantを作れない。未作成suffix・symlink・dotdot・別実行ファイルでも許可外アクセスが拒否される |
| 2 | F4/F10: executorとcancel | 大量stdout/stderrで停止しない。timeout/cancelで子孫を停止。signal終了を成功扱いしない。Escが本当に中断する |
| 3 | F5/F7: step記録とstream契約 | API失敗後も実行済みtoolが再開時に見える。未完streamからツールを実行しない。length/error/cancelを区別 |
| 4 | F6/F11: contextとsession | 25件の小会話を不要に圧縮しない。tool-resultが孤立しない。requestをprovider contractで検証。保存途中から回復できる |
| 5 | F8/F9: 編集とGit | dry-runとapplyが同じplanを使う。部分失敗が明示される。指定外の変更をcommitしない |
| 6 | Project instructions＋設定整理 | root/nested AGENTSのscope、出典、上限、再読込が試験可能。実設定をUIに表示 |
| 7 | 第二provider＋headless | 同じfixtureをAnthropicとOpenAI-compatible adapterへ投影し契約試験。JSONLで自動評価できる |
| 8 | Eval＋必要なtool/UX改善 | 同じmodel・予算で成功率、回帰、介入、費用を比較し、改善を示せる |
| 9 | 需要が確認できた拡張 | MCP・skills高度化・plan・subagents・Web UIを個別に評価して追加 |

これは工程規模の見積りではなく依存関係の提案。executorとsessionを一度に巨大化せず、再現テストを修正後の期待へ反転して、小さい変更ごとに保証を増やすのがよい。

**評価方法も製品の一部にする。**

既存108テストの成功を否定する必要はない。ただし、それだけでは「何が起きてはいけないか」を十分表現していない。以下の3層を分ける。

| 層 | 内容 | 合格条件の例 |
|---|---|---|
| 決定論的な契約試験 | tool schema、message役割、stop理由、pair整合、permission、path | 不正なgrant／message／tool引数を実行前に拒否 |
| 故障注入試験 | 2回目API失敗、stream EOF、disk write失敗、worker panic、child孫、途中再起動 | 実行済み事実が失われず、停止と復旧が説明可能 |
| 実モデル評価 | repo理解、bug fix、小refactor、tests追加、project規約、長い会話 | 独立したhidden testsと人間の差分確認で成功判定 |

最初の実モデル評価セットは20〜30タスク程度から始められる。タスクごとのベースcommitと期待を固定し、ユーザーの既存変更、同名hunk、非ASCII、長いtool結果、失敗したbuild、途中での要件変更を含める。数値は提案であり、今回実施したベンチマーク件数ではない。

pi/dsh/sociumの比較は可能な限り同じmodel、provider endpoint、sampling設定、入力task、context/output予算、ネットワーク制約に揃える。harness既定の比較と、tool構成を揃えた比較を分ける。モデル相性の差を設計の優劣と混ぜず、複数seed/反復のばらつきも残す。

指標は成功率だけでなく、意図しないファイル変更、変更行数、必要な人間介入、API/tool失敗率、復旧率、token費用、時間、失敗の診断可能性。自己申告の「完了」ではなく、外部のvalidatorで判断する。最低限の失敗契約を実装するためのheadless runnerは、豪華なTUIや新toolより先に価値が出る。

**再現検証の一覧。**

| 観察用テスト | 確認できたこと |
|---|---|
| model_supplied_approval_is_sufficient | JSONのapprovalだけで一時ファイルを削除できる |
| safe_read_only_sed_can_write_without_approval | read-only表示のままsed -iが書き換える |
| command_reads_outside_workspace | 絶対パスのcatがworkspace外の非機密fixtureを読む |
| new_path_dotdot_escapes_workspace | 新規pathのmissing/../../がworkspace外へ作成する |
| pipe_output_causes_false_timeout | 1 MiBのcatがpipe詰まりでtimeoutになる |
| child_inherited_pipe_outlives_timeout | 子孫のpipe保持で指定timeoutを超える |
| signal_death_reported_as_success | SIGTERM終了をtool成功にする |
| second_request_failure_loses_record_of_completed_edit | ファイル変更済みなのに会話／保存に結果が残らない |
| small_history_is_summarized_and_starts_with_system | 25件の短い履歴で不要な要約と先頭systemが生じる |
| emergency_context_cut_orphans_tool_result | tool_useを要約しtool_resultだけ残す |
| partial_patch_keeps_first_write_after_second_fails | 後半actionの失敗後も前半の書込みが残る |
| dry_run_accepts_missing_update_and_unchecked_destination | 存在しないUpdateでもdry-runだけ成功する（この試験が実行するのはmissing Update条件） |
| git_commit_includes_unrelated_unstaged_files | 未stageの無関係なファイルをcommitへ混入する |
| incomplete_stream_can_finish_with_executable_tool | message_stop未受信のbuilderがtool blockを返す |
| token_limit_is_discarded_by_stream_builder | max_tokens理由がresponseから消える |

再実行例（書込可能な依存キャッシュを利用する場合）:

```bash
CARGO_HOME=/tmp/socium-review-cargo python3 /home/chansuke/code/myprojects/socium/docs/review-evidence/2026-11-11/run_repros.py
```

ランナーは一時ソースコピーにだけ試験を追加する。今後実装を修正すると、これらの「欠陥がある」assertionは失敗する。その時点で通常の回帰テストへ移し、期待を安全な振る舞いに反転する必要がある。

**最終評価。**
sociumの方向性は、小さく、把握でき、改造しやすいagent coreとして成立している。現時点では個人の実験・実装学習の土台として評価できるが、重要な未commit変更を抱えたrepoで任せきりにする信頼性には達していない。これはRustや小規模設計の限界ではなく、具体的な実行・保存契約がまだ未完成なためである。

最も有望な改善は、piの少ない既定ツールと厚い実行基盤を参考にし、dshから承認・executor・session logの責任分離だけを取り込むこと。差別化候補は「小さいRust binary」「可搬な実行基盤」「local modelで測定可能な品質」「中断と再開の確実さ」であり、いずれも現時点の実証済み優位性ではない。これを測れる形にすることが、機能一覧を増やすより先に必要だ。
