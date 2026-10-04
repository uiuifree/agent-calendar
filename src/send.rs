//! 画面から続きの指示を送る。`claude -p --resume` / `codex exec resume` を裏で動かし、途中経過を 1 行ずつ返す。
//! 送った指示も返答も、元のセッションの記録にそのまま追記される（同じセッション ID のまま）
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// 1 回の実行の上限。超えたら止める
pub const TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// エラー出力は最後のここまでだけ残す（失敗の理由を見せるため）
const STDERR_TAIL: usize = 4096;
pub const MAX_PROMPT_CHARS: usize = 20_000;
/// 一緒に送れる画像（貼り付けたスクリーンショットなど）の枚数と、1 枚の大きさの上限
pub const MAX_IMAGES: usize = 5;
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// 画面から受け取る画像。data は base64
#[derive(Debug, Clone, Deserialize)]
pub struct Image {
    pub media_type: String,
    pub data: String,
}

/// 受け取った画像を確かめる（形式・大きさ・枚数、中身の先頭が形式と合うか）。戻り値は (拡張子, 中身)
pub fn check_images(images: &[Image]) -> Result<Vec<(&'static str, Vec<u8>)>> {
    use base64::Engine;
    if images.len() > MAX_IMAGES {
        bail!("up to {MAX_IMAGES} images");
    }
    let mut out = Vec::new();
    for i in images {
        let (ext, magic): (&str, &[u8]) = match i.media_type.as_str() {
            "image/png" => ("png", b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => ("jpg", b"\xff\xd8\xff"),
            "image/gif" => ("gif", b"GIF8"),
            "image/webp" => ("webp", b"RIFF"),
            other => bail!("unsupported image type: {other}"),
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&i.data)
            .map_err(|_| anyhow::anyhow!("an image is not valid base64"))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            bail!(
                "each image must be {} MB or smaller",
                MAX_IMAGE_BYTES / 1024 / 1024
            );
        }
        if !bytes.starts_with(magic) || (ext == "webp" && bytes.get(8..12) != Some(b"WEBP")) {
            bail!("an image does not match its type ({})", i.media_type);
        }
        out.push((ext, bytes));
    }
    Ok(out)
}

/// 画像と、許可の問い合わせの受け方を付ける。
/// Claude は指示を JSON（stream-json）で渡して画像をそのまま載せる。ask なら許可の問い合わせを
/// こちら（画面）で受けると宣言する（`--permission-prompt-tool stdio` と最初の initialize）。
/// Codex は画像をファイルに書いて `--image` で渡す（書いたファイルは終わったら消す。Codex には問い合わせの仕組みが無い）。
/// 画像も問い合わせも無ければ引数も指示もそのまま。戻り値は (標準入力に渡す指示, 終わったら消すファイル)
pub fn attach(
    source: &str,
    args: &mut Vec<String>,
    prompt: &str,
    (images, checked): (&[Image], &[(&str, Vec<u8>)]),
    dir: &std::path::Path,
    ask: bool,
) -> Result<(String, Vec<std::path::PathBuf>)> {
    if source != "codex" {
        if images.is_empty() && !ask {
            return Ok((format!("{prompt}\n"), Vec::new()));
        }
        args.extend(["--input-format".to_string(), "stream-json".to_string()]);
        let mut stdin = String::new();
        if ask {
            args.extend(["--permission-prompt-tool".to_string(), "stdio".to_string()]);
            let init = json!({ "type": "control_request", "request_id": "init", "request": { "subtype": "initialize" } });
            stdin.push_str(&format!("{init}\n"));
        }
        let mut content = vec![json!({ "type": "text", "text": prompt })];
        content.extend(images.iter().map(|i| {
            json!({ "type": "image", "source": { "type": "base64", "media_type": i.media_type, "data": i.data } })
        }));
        let line = json!({ "type": "user", "message": { "role": "user", "content": content } });
        stdin.push_str(&format!("{line}\n"));
        return Ok((stdin, Vec::new()));
    }
    if images.is_empty() {
        return Ok((format!("{prompt}\n"), Vec::new()));
    }
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let mut files = Vec::new();
    for (ext, bytes) in checked {
        let mut name = [0u8; 8];
        getrandom::fill(&mut name).map_err(|e| anyhow::anyhow!("{e}"))?;
        let hex: String = name.iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join(format!("{hex}.{ext}"));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        std::io::Write::write_all(&mut f, bytes)?;
        args.push(format!("--image={}", path.display()));
        files.push(path);
    }
    Ok((format!("{prompt}\n"), files))
}

/// 画面から選べる許可の範囲。確認を全部飛ばすモード（bypassPermissions など）はブラウザからは選ばせない
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// 読むだけ（Claude は plan、Codex は read-only）
    Read,
    /// ファイルの編集まで（Claude は acceptEdits、Codex は workspace-write）
    Edit,
    /// 自動判定（Claude は auto: 操作ごとに安全かを判定させる。Codex には無いので workspace-write）
    Auto,
}

fn claude_mode(mode: Mode) -> &'static str {
    match mode {
        Mode::Read => "plan",
        Mode::Edit => "acceptEdits",
        Mode::Auto => "auto",
    }
}

fn codex_sandbox(mode: Mode) -> &'static str {
    match mode {
        Mode::Read => "read-only",
        Mode::Edit | Mode::Auto => "workspace-write",
    }
}

/// 実行するプログラムと引数。指示は引数に載せず標準入力で渡す（長さの制限と、プロセス一覧から見えるのを避ける）
pub fn command(source: &str, id: &str, mode: Mode) -> (&'static str, Vec<String>) {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    if source == "codex" {
        let sandbox = codex_sandbox(mode);
        let mut args = s(&[
            "exec",
            "resume",
            id,
            "-",
            "--json",
            "--skip-git-repo-check",
            "-c",
        ]);
        args.push(format!("sandbox_mode=\"{sandbox}\""));
        ("codex", args)
    } else {
        let perm = claude_mode(mode);
        (
            "claude",
            s(&[
                "-p",
                "--resume",
                id,
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                perm,
            ]),
        )
    }
}

/// モデルの名前として受けてよい形か（`opus` のような別名か、`claude-opus-4-1` のような正式な名前）。
/// `-` で始まるものは CLI の別のオプションとして読まれるので断る。その名前のモデルがあるかは CLI が決める
pub fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 100
        && !model.starts_with('-')
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/-[]".contains(c))
}

/// モデルを選ばずに始めたときに CLI が使うモデル（本人の設定に書いてあれば）。画面の「既定」に添えて見せる。
/// Claude Code は `~/.claude/settings.json` の model、Codex は `~/.codex/config.toml` の先頭の model。
/// 書いていなければ None（CLI の側の既定になるので、こちらからは分からない）
pub fn cli_default_model(source: &str, home: &std::path::Path) -> Option<String> {
    let model = if source == "codex" {
        let text = std::fs::read_to_string(home.join(".codex/config.toml")).ok()?;
        text.lines()
            .take_while(|l| !l.trim_start().starts_with('['))
            .find_map(|l| {
                let (key, value) = l.split_once('=')?;
                (key.trim() == "model").then(|| value.trim().trim_matches('"').to_string())
            })?
    } else {
        let text = std::fs::read_to_string(home.join(".claude/settings.json")).ok()?;
        serde_json::from_str::<Value>(&text).ok()?["model"]
            .as_str()?
            .to_string()
    };
    valid_model(&model).then_some(model)
}

/// 新しいセッションを始めるときの起動のしかた（「ここで始める」と予定の実行で使う）。
/// model が空なら CLI の既定のモデルに任せる
pub fn command_new(source: &str, mode: Mode, model: &str) -> (&'static str, Vec<String>) {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let (program, mut args) = if source == "codex" {
        let sandbox = codex_sandbox(mode);
        let mut args = s(&["exec", "-", "--json", "--skip-git-repo-check", "-c"]);
        args.push(format!("sandbox_mode=\"{sandbox}\""));
        ("codex", args)
    } else {
        let perm = claude_mode(mode);
        (
            "claude",
            s(&[
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                perm,
            ]),
        )
    };
    if !model.is_empty() {
        args.extend(s(&["--model", model]));
    }
    (program, args)
}

/// 許可の問い合わせ（control_request の can_use_tool）を画面に出す形にする
fn permission_event(v: &Value) -> Value {
    let r = &v["request"];
    let input = &r["input"];
    let text = input["command"]
        .as_str()
        .or(input["file_path"].as_str())
        .or(r["description"].as_str())
        .unwrap_or_default();
    let rule = remember_rule(r["tool_name"].as_str().unwrap_or_default(), input);
    json!({
        "kind": "permission",
        "request_id": v["request_id"],
        "tool": r["display_name"].as_str().or(r["tool_name"].as_str()).unwrap_or_default(),
        "text": text,
        "rule": rule_label(&rule),
    })
}

/// claude / codex の途中経過を、画面が使う形（session / text / tool / done）にそろえる。
/// session はセッション ID（新しく始めたときに、続きを送るため覚えておく）
pub fn normalize(source: &str, line: &str) -> Vec<Value> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    if source == "codex" {
        return match v["type"].as_str() {
            Some("thread.started") => vec![json!({ "kind": "session", "id": v["thread_id"] })],
            Some("item.completed") => {
                let it = &v["item"];
                match it["type"].as_str() {
                    Some("agent_message") => vec![json!({ "kind": "text", "text": it["text"] })],
                    Some("command_execution") => {
                        vec![json!({ "kind": "tool", "name": "shell", "text": it["command"] })]
                    }
                    Some("reasoning") | None => Vec::new(),
                    Some(other) => vec![json!({ "kind": "tool", "name": other, "text": "" })],
                }
            }
            Some("turn.completed") => vec![json!({ "kind": "done", "ok": true })],
            Some("turn.failed") | Some("error") => {
                let msg = v["error"]["message"]
                    .as_str()
                    .or(v["message"].as_str())
                    .unwrap_or("failed");
                vec![json!({ "kind": "done", "ok": false, "text": msg })]
            }
            _ => Vec::new(),
        };
    }
    match v["type"].as_str() {
        // 許可の問い合わせ（画面で受けると宣言したときだけ届く）
        Some("control_request") if v["request"]["subtype"] == "can_use_tool" => {
            vec![permission_event(&v)]
        }
        Some("system") if v["subtype"] == "init" => vec![json!({ "kind": "session", "id": v["session_id"] })],
        Some("assistant") => v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| match b["type"].as_str() {
                Some("text") => Some(json!({ "kind": "text", "text": b["text"] })),
                Some("tool_use") => Some(json!({
                    "kind": "tool",
                    "name": b["name"],
                    "text": b["input"]["command"].as_str().or(b["input"]["file_path"].as_str()).unwrap_or(""),
                })),
                _ => None,
            })
            .collect(),
        Some("result") => {
            let ok = v["is_error"] != true && v["subtype"] == "success";
            vec![json!({ "kind": "done", "ok": ok, "text": if ok { Value::Null } else { v["result"].clone() } })]
        }
        _ => Vec::new(),
    }
}

/// 2 語目までがコマンドの一部になるもの（`cargo test` と `cargo publish` は分けて許可したい）
const TWO_WORD: &[&str] = &[
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "npx",
    "git",
    "gh",
    "go",
    "docker",
    "make",
    "uv",
    "pip",
    "pip3",
    "rustup",
    "kubectl",
    "terraform",
    "gcloud",
    "aws",
];

/// 「このリポジトリでは今後も許可」で足すルール。Bash はコマンドの先頭（`Bash(cargo test:*)`）、
/// ほかのツールはツールの名前だけ。戻り値は (ツール, ルールの中身)
pub fn remember_rule(tool: &str, input: &Value) -> (String, Option<String>) {
    if tool != "Bash" {
        return (tool.to_string(), None);
    }
    let words: Vec<&str> = input["command"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .take(2)
        .collect();
    let prefix = match words.as_slice() {
        [first, second, ..] if TWO_WORD.contains(first) && !second.starts_with('-') => {
            format!("{first} {second}")
        }
        [first, ..] => first.to_string(),
        [] => return (tool.to_string(), None),
    };
    (tool.to_string(), Some(format!("{prefix}:*")))
}

/// 画面に出すルールの書き方（`Bash(cargo test:*)`）
pub fn rule_label((tool, content): &(String, Option<String>)) -> String {
    match content {
        Some(c) => format!("{tool}({c})"),
        None => tool.clone(),
    }
}

/// 実行中のセッション。同じセッションに同時に 2 つ送らない（記録に 2 か所から書くことになる）。
/// あわせて、画面の答えを待っている許可の問い合わせ（問い合わせの ID → その実行の標準入力と、問われた操作）を持つ
#[derive(Default, Clone)]
pub struct Running {
    set: Arc<Mutex<HashSet<String>>>,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
    /// 本体を入れ替えている最中。新しい実行を受け付けない（入れ替えのあとの再起動で途中で切れるので）
    updating: Arc<std::sync::atomic::AtomicBool>,
}

/// 答えを待っている許可の問い合わせ
struct Pending {
    stdin: mpsc::Sender<String>,
    input: Value,
    /// 「今後も許可」で足すルール
    rule: (String, Option<String>),
    /// どのセッションの問い合わせか（画面を開き直したときに出し直すため）と、画面に出す形
    session: String,
    event: Value,
}

impl Running {
    /// 印を付ける。すでに付いていれば None。戻り値を手放すと印が外れる
    pub fn claim(&self, id: &str) -> Option<Claim> {
        let mut set = self.set.lock().unwrap_or_else(|e| e.into_inner());
        // 入れ替えの判定と同じ鍵の中で見る（判定と新しい実行が入れ違わないように）
        if self.updating() {
            return None;
        }
        set.insert(id.to_string()).then(|| Claim {
            set: self.set.clone(),
            id: id.to_string(),
        })
    }

    pub fn updating(&self) -> bool {
        self.updating.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 本体の入れ替えを始める。何も動いていなければ、以後の新しい実行を止めて true。
    /// 動いていれば何も変えずに false。判定と止める切り替えは、実行の印と同じ鍵の中で行う
    pub fn begin_update(&self) -> bool {
        let set = self.set.lock().unwrap_or_else(|e| e.into_inner());
        let idle = set.is_empty()
            && self
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty();
        if idle {
            self.updating
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        idle
    }

    /// 入れ替えをやめた・再起動しないときに、実行の受け付けを戻す
    pub fn end_update(&self) {
        self.updating
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// 何も動いていない（実行中の指示も、答えを待っている問い合わせも無い）。入れ替えて再起動してよいか
    pub fn idle(&self) -> bool {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
            && self
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(id)
    }

    /// 画面で押した許可・拒否を Claude に返す。remember なら、そのルールをリポジトリの
    /// `.claude/settings.local.json` に足させる（次からは聞かれない）。もう終わった・答えた問い合わせなら Err
    pub fn answer(&self, request_id: &str, allow: bool, remember: bool) -> Result<()> {
        let Some(p) = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id)
        else {
            bail!("this permission request is no longer waiting");
        };
        let decision = if allow && remember {
            let (tool, content) = &p.rule;
            let mut rule = json!({ "toolName": tool });
            if let Some(c) = content {
                rule["ruleContent"] = json!(c);
            }
            json!({
                "behavior": "allow",
                "updatedInput": p.input,
                "updatedPermissions": [{ "type": "addRules", "rules": [rule], "behavior": "allow", "destination": "localSettings" }],
            })
        } else if allow {
            json!({ "behavior": "allow", "updatedInput": p.input })
        } else {
            json!({ "behavior": "deny", "message": "The user denied this from the agent-calendar page." })
        };
        let line = json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": decision },
        });
        p.stdin
            .try_send(format!("{line}\n"))
            .map_err(|_| anyhow::anyhow!("the run has already finished"))
    }

    fn wait_for(&self, session: &str, stdin: mpsc::Sender<String>, v: &Value) {
        let request = &v["request"];
        let tool = request["tool_name"].as_str().unwrap_or_default();
        let request_id = v["request_id"].as_str().unwrap_or_default();
        let pending = Pending {
            stdin,
            input: request["input"].clone(),
            rule: remember_rule(tool, &request["input"]),
            session: session.to_string(),
            event: permission_event(v),
        };
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(request_id.to_string(), pending);
    }

    /// そのセッションで、画面の答えを待っている問い合わせ（会話の画面を開き直したときに出し直す）
    pub fn asks(&self, session: &str) -> Vec<Value> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|p| p.session == session)
            .map(|p| p.event.clone())
            .collect()
    }

    /// 実行が終わったら、その実行の答えを待っていた問い合わせを捨てる
    fn forget(&self, ids: &[String]) {
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for id in ids {
            p.remove(id);
        }
    }
}

/// 終わったら（途中で落ちても）印を外す
pub struct Claim {
    set: Arc<Mutex<HashSet<String>>>,
    id: String,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

/// 起動して、途中経過を 1 行ずつ（NDJSON）流す受け口を返す。`after` は終わったあとに 1 回呼ぶ（読み直し）。
/// 画面を閉じても実行は止めない（編集の途中で止めるほうが危ない）
/// 1 回の実行に要るもの
pub struct Job {
    pub program: String,
    pub args: Vec<String>,
    /// claude / codex（途中経過の読み方が違う）
    pub source: String,
    pub id: String,
    pub cwd: String,
    pub prompt: String,
    /// これを超えたら止める（通常は TIMEOUT）
    pub timeout: Duration,
    /// 終わったら消すファイル（Codex に渡した画像）
    pub cleanup: Vec<std::path::PathBuf>,
    /// 許可の問い合わせを画面で受ける（attach に ask を渡したとき）。標準入力を閉じずに答えを書き足す
    pub ask: bool,
}

pub fn start(
    running: &Running,
    job: Job,
    after: impl FnOnce() + Send + 'static,
) -> Result<mpsc::Receiver<String>> {
    // 起動できなかったときも、渡すはずだった画像は消す
    let discard = |files: &[std::path::PathBuf]| {
        for f in files {
            let _ = std::fs::remove_file(f);
        }
    };
    let Some(claim) = running.claim(&job.id) else {
        discard(&job.cleanup);
        if running.updating() {
            bail!("Agent Calendar is updating; try again in a minute");
        }
        bail!("an instruction is already running for this session");
    };
    let spawned = tokio::process::Command::new(&job.program)
        .args(&job.args)
        .current_dir(&job.cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            discard(&job.cleanup);
            return Err(e.into());
        }
    };
    let (tx, rx) = mpsc::channel::<String>(256);
    let running = running.clone();
    let (source, prompt, limit, cleanup) = (job.source, job.prompt, job.timeout, job.cleanup);
    let ask = job.ask && source != "codex";
    let job_id = job.id.clone();
    tokio::spawn(async move {
        // 問い合わせを画面で受けるときは標準入力を閉じずに、答えをここから書き足す（結果が出たら閉じる）
        let mut answers: Option<mpsc::Sender<String>> = None;
        if let Some(mut stdin) = child.stdin.take() {
            if ask {
                let (in_tx, mut in_rx) = mpsc::channel::<String>(16);
                let _ = in_tx.try_send(prompt);
                tokio::spawn(async move {
                    while let Some(line) = in_rx.recv().await {
                        if stdin.write_all(line.as_bytes()).await.is_err() {
                            break;
                        }
                        let _ = stdin.flush().await;
                    }
                });
                answers = Some(in_tx);
            } else {
                let _ = stdin.write_all(prompt.as_bytes()).await;
            }
        }
        // エラー出力は並行して読む（読まずにいるとパイプが詰まって CLI が止まる）。最後の 4KB だけ残す
        let stderr = child.stderr.take().map(|mut e| {
            tokio::spawn(async move {
                let (mut tail, mut buf) = (Vec::new(), [0u8; 4096]);
                while let Ok(n) = e.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > STDERR_TAIL {
                        tail.drain(..tail.len() - STDERR_TAIL);
                    }
                }
                String::from_utf8_lossy(&tail).into_owned()
            })
        });
        let mut lines = BufReader::new(child.stdout.take().expect("stdout is piped")).lines();
        let mut done = false;
        // 新しく始めたセッションは、ID が分かった時点でその ID にも印を付ける（画面から同じセッションへ並行して送らせない）
        let mut session_claim = None;
        // この実行で画面の答えを待っている問い合わせと、いまのセッション ID（新しく始めたときは session で分かる）
        let mut asked: Vec<String> = Vec::new();
        let mut current = job_id;
        let read = async {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(in_tx) = &answers
                    && let Ok(v) = serde_json::from_str::<Value>(&line)
                    && v["type"] == "control_request"
                {
                    let id = v["request_id"].as_str().unwrap_or_default().to_string();
                    if v["request"]["subtype"] == "can_use_tool" {
                        running.wait_for(&current, in_tx.clone(), &v);
                        asked.push(id);
                    } else {
                        // 許可のほかの問い合わせ（フックなど）は受けていないと返す（返さないと Claude が待ち続ける）
                        let no = json!({
                            "type": "control_response",
                            "response": { "subtype": "error", "request_id": id, "error": "not supported by agent-calendar" },
                        });
                        let _ = in_tx.send(format!("{no}\n")).await;
                    }
                }
                for ev in normalize(&source, &line) {
                    done |= ev["kind"] == "done";
                    if done {
                        // 結果が出たら標準入力を閉じて終わらせる。答えを待っていた問い合わせも捨てる
                        answers = None;
                        running.forget(&asked);
                    }
                    if ev["kind"] == "session"
                        && session_claim.is_none()
                        && let Some(id) = ev["id"].as_str()
                    {
                        session_claim = running.claim(id);
                        current = id.to_string();
                    }
                    let _ = tx.send(format!("{ev}\n")).await;
                }
            }
        };
        // 時間切れなら止めてから回収する（止めないと編集が上限を超えて続く）
        let timed_out = tokio::time::timeout(limit, read).await.is_err();
        if timed_out {
            let _ = child.start_kill();
        }
        let status = child.wait().await;
        // 孫プロセスがエラー出力を握ったままのこともあるので、待つのは少しだけ
        let err = match stderr {
            Some(t) => tokio::time::timeout(Duration::from_secs(5), t)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default(),
            None => String::new(),
        };
        if timed_out || !done {
            let why = match status {
                _ if timed_out => format!("stopped after {} minutes", limit.as_secs() / 60),
                Ok(s) => format!("exited with {s}: {}", err.trim()),
                Err(e) => e.to_string(),
            };
            let _ = tx
                .send(format!(
                    "{}\n",
                    json!({ "kind": "done", "ok": false, "text": why })
                ))
                .await;
        }
        running.forget(&asked);
        discard(&cleanup);
        drop(session_claim);
        drop(tx);
        // 終わった時点で印を外す（読み直しを待たずに次の指示を送れるように）
        drop(claim);
        let _ = tokio::task::spawn_blocking(after).await;
    });
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(media_type: &str, bytes: &[u8]) -> Image {
        use base64::Engine;
        Image {
            media_type: media_type.into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n....";

    #[test]
    fn checks_images() {
        let ok = check_images(&[
            img("image/png", PNG),
            img("image/jpeg", b"\xff\xd8\xff\xe0"),
            img("image/gif", b"GIF89a"),
            img("image/webp", b"RIFF\0\0\0\0WEBPVP8 "),
        ])
        .unwrap();
        let exts: Vec<_> = ok.iter().map(|(e, _)| *e).collect();
        assert_eq!(exts, ["png", "jpg", "gif", "webp"]);
        assert_eq!(ok[0].1, PNG);
        assert!(check_images(&vec![img("image/png", PNG); MAX_IMAGES + 1]).is_err());
        assert!(check_images(&[img("image/svg+xml", b"<svg/>")]).is_err());
        assert!(check_images(&[img("image/png", b"GIF89a")]).is_err()); // 中身が形式と合わない
        assert!(check_images(&[img("image/webp", b"RIFF\0\0\0\0AVI ")]).is_err());
        let bad = Image {
            media_type: "image/png".into(),
            data: "%%%".into(),
        };
        assert!(check_images(&[bad]).is_err());
        let mut big = PNG.to_vec();
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        assert!(check_images(&[img("image/png", &big)]).is_err());
    }

    #[test]
    fn attaches_images() {
        let dir = crate::db::temp_dir("send-attach");
        // 画像が無ければ、引数も指示もそのまま（予定の実行と同じ）
        let mut args = vec!["-p".to_string()];
        let (stdin, files) = attach("claude", &mut args, "見て", (&[], &[]), &dir, false).unwrap();
        assert_eq!((stdin.as_str(), args.len(), files.len()), ("見て\n", 1, 0));
        let (stdin, _) = attach("codex", &mut args, "見て", (&[], &[]), &dir, true).unwrap();
        assert_eq!((stdin.as_str(), args.len()), ("見て\n", 1)); // Codex に問い合わせの仕組みは無い

        let images = [img("image/png", PNG)];
        let checked = check_images(&images).unwrap();
        let (stdin, files) = attach(
            "claude",
            &mut args,
            "見て",
            (&images, &checked),
            &dir,
            false,
        )
        .unwrap();
        assert_eq!(args[1..], ["--input-format", "stream-json"]);
        assert!(files.is_empty());
        let line: Value = serde_json::from_str(stdin.trim_end()).unwrap();
        let content = &line["message"]["content"];
        assert_eq!(content[0]["text"], "見て");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], images[0].data);

        // 問い合わせを画面で受けるときは、最初に initialize を送る
        let mut args = vec!["-p".to_string()];
        let (stdin, _) = attach("claude", &mut args, "見て", (&[], &[]), &dir, true).unwrap();
        assert_eq!(
            args[1..],
            [
                "--input-format",
                "stream-json",
                "--permission-prompt-tool",
                "stdio"
            ]
        );
        let lines: Vec<Value> = stdin
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["request"]["subtype"], "initialize");
        assert_eq!(lines[1]["message"]["content"][0]["text"], "見て");

        let mut args = vec!["exec".to_string()];
        let (stdin, files) =
            attach("codex", &mut args, "見て", (&images, &checked), &dir, false).unwrap();
        assert_eq!(stdin, "見て\n");
        assert_eq!(files.len(), 1);
        assert_eq!(args[1], format!("--image={}", files[0].display()));
        assert_eq!(std::fs::read(&files[0]).unwrap(), PNG);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&files[0]).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn commands_allow_only_two_modes() {
        let (p, a) = command("claude", "s1", Mode::Read);
        assert_eq!(p, "claude");
        assert_eq!(
            a,
            [
                "-p",
                "--resume",
                "s1",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "plan"
            ]
        );
        assert_eq!(
            command("claude", "s1", Mode::Edit).1.last().unwrap(),
            "acceptEdits"
        );
        let (p, a) = command("codex", "t1", Mode::Edit);
        assert_eq!(p, "codex");
        assert_eq!(a[..4], ["exec", "resume", "t1", "-"]);
        assert_eq!(a.last().unwrap(), "sandbox_mode=\"workspace-write\"");
        assert!(
            command("codex", "t1", Mode::Read)
                .1
                .last()
                .unwrap()
                .contains("read-only")
        );
        assert!(serde_json::from_str::<Mode>("\"bypass\"").is_err());
        // 自動判定は Claude の auto。Codex には無いので編集まで（workspace-write）と同じ
        assert_eq!(
            command("claude", "s1", Mode::Auto).1.last().unwrap(),
            "auto"
        );
        assert_eq!(
            command_new("claude", Mode::Auto, "").1.last().unwrap(),
            "auto"
        );
        // モデルを選んだときだけ --model を付ける（選ばなければ CLI の既定）
        for agent in ["claude", "codex"] {
            let a = command_new(agent, Mode::Edit, "opus").1;
            assert_eq!(a[a.len() - 2..], ["--model", "opus"], "{agent}");
            assert!(
                !command_new(agent, Mode::Edit, "")
                    .1
                    .contains(&"--model".to_string())
            );
        }
        assert!(valid_model("opus") && valid_model("claude-opus-4-1") && valid_model("opus[1m]"));
        assert!(!valid_model("") && !valid_model("--dangerously-skip-permissions"));
        assert!(!valid_model("a b") && !valid_model(&"x".repeat(101)));
        // CLI の既定のモデルは本人の設定から読む。書いていない・読めないなら分からない
        let home = crate::db::temp_dir("cli-default");
        assert_eq!(cli_default_model("claude", &home), None);
        assert_eq!(cli_default_model("codex", &home), None);
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".claude/settings.json"), r#"{"model":"fable"}"#).unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            "approval = 1\nmodel = \"gpt-x\"\n[profiles.a]\nmodel = \"other\"\n",
        )
        .unwrap();
        assert_eq!(cli_default_model("claude", &home).as_deref(), Some("fable"));
        assert_eq!(cli_default_model("codex", &home).as_deref(), Some("gpt-x"));
        std::fs::write(home.join(".claude/settings.json"), r#"{"model":"--x"}"#).unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            "[profiles.a]\nmodel = \"other\"\n",
        )
        .unwrap();
        assert_eq!(cli_default_model("claude", &home), None);
        assert_eq!(cli_default_model("codex", &home), None);
        assert!(
            command_new("codex", Mode::Auto, "")
                .1
                .last()
                .unwrap()
                .contains("workspace-write")
        );
        assert_eq!(
            serde_json::from_str::<Mode>("\"auto\"").unwrap(),
            Mode::Auto
        );
        assert_eq!(
            command_new("claude", Mode::Read, "").1,
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "plan"
            ]
        );
        let (p, a) = command_new("codex", Mode::Edit, "");
        assert_eq!(
            (p, a[..2].to_vec()),
            ("codex", vec!["exec".to_string(), "-".to_string()])
        );
        assert!(a.last().unwrap().contains("workspace-write"));
        assert_eq!(
            serde_json::from_str::<Mode>("\"edit\"").unwrap(),
            Mode::Edit
        );
    }

    #[test]
    fn normalizes_claude_events() {
        let a = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"見ます"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}},{"type":"thinking"}]}}"#;
        assert_eq!(
            normalize("claude", a),
            [
                json!({"kind":"text","text":"見ます"}),
                json!({"kind":"tool","name":"Bash","text":"ls"})
            ]
        );
        assert_eq!(
            normalize(
                "claude",
                r#"{"type":"result","subtype":"success","is_error":false}"#
            ),
            [json!({"kind":"done","ok":true,"text":null})]
        );
        assert_eq!(
            normalize(
                "claude",
                r#"{"type":"result","subtype":"error_max_turns","is_error":true,"result":"x"}"#
            ),
            [json!({"kind":"done","ok":false,"text":"x"})]
        );
        assert!(normalize("claude", r#"{"type":"system","subtype":"hook_started"}"#).is_empty());
        assert_eq!(
            normalize(
                "claude",
                r#"{"type":"system","subtype":"init","session_id":"s9"}"#
            ),
            [json!({"kind":"session","id":"s9"})]
        );
        assert!(normalize("claude", "not json").is_empty());
    }

    #[test]
    fn normalizes_codex_events() {
        let n = |l: &str| normalize("codex", l);
        assert_eq!(
            n(r#"{"type":"item.completed","item":{"type":"agent_message","text":"two"}}"#),
            [json!({"kind":"text","text":"two"})]
        );
        assert_eq!(
            n(
                r#"{"type":"item.completed","item":{"type":"command_execution","command":"cargo test"}}"#
            )[0]["text"],
            "cargo test"
        );
        assert_eq!(
            n(r#"{"type":"item.completed","item":{"type":"file_change"}}"#)[0]["name"],
            "file_change"
        );
        assert!(n(r#"{"type":"item.completed","item":{"type":"reasoning"}}"#).is_empty());
        assert_eq!(
            n(r#"{"type":"turn.completed"}"#),
            [json!({"kind":"done","ok":true})]
        );
        assert_eq!(
            n(r#"{"type":"turn.failed","error":{"message":"boom"}}"#)[0]["text"],
            "boom"
        );
        assert!(n(r#"{"type":"turn.started"}"#).is_empty());
        assert_eq!(
            n(r#"{"type":"thread.started","thread_id":"t9"}"#),
            [json!({"kind":"session","id":"t9"})]
        );
    }

    fn fake(dir: &std::path::Path, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(format!("fake-{}", body.len()));
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// 書いたばかりのスクリプトを直接実行すると、並行するテストのプロセス分岐と重なって
    /// まれに「Text file busy」になるので、sh に読ませる（存在しないプログラムの確かめだけは直接）
    fn job(program: &str, id: &str, dir: &std::path::Path, prompt: String) -> Job {
        let (program, args) = if program.starts_with("/nonexistent") {
            (program.to_string(), vec![])
        } else {
            ("/bin/sh".to_string(), vec![program.to_string()])
        };
        Job {
            program,
            args,
            source: "claude".into(),
            id: id.into(),
            cwd: dir.to_string_lossy().into_owned(),
            prompt,
            timeout: TIMEOUT,
            cleanup: Vec::new(),
            ask: false,
        }
    }

    async fn collect(mut rx: mpsc::Receiver<String>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Some(l) = rx.recv().await {
            out.push(serde_json::from_str(l.trim()).unwrap());
        }
        out
    }

    #[tokio::test]
    async fn streams_events_and_releases_the_lock() {
        let dir = crate::db::temp_dir("send");
        let running = Running::default();
        // 指示は標準入力で届く。それをそのまま返答として返す
        let ok = fake(
            &dir,
            r#"read p; printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s"}]}}\n' "$p"; echo '{"type":"result","subtype":"success","is_error":false}'"#,
        );
        let (tx, rx_after) = std::sync::mpsc::channel();
        let rx = start(
            &running,
            job(&ok, "s1", &dir, "hello\n".into()),
            move || tx.send(()).unwrap(),
        )
        .unwrap();
        assert!(running.contains("s1"));
        // 実行中は同じセッションに送れない
        assert!(start(&running, job(&ok, "s1", &dir, "x".into()), || {}).is_err());
        let ev = collect(rx).await;
        assert_eq!(
            ev,
            [
                json!({"kind":"text","text":"hello"}),
                json!({"kind":"done","ok":true,"text":null})
            ]
        );
        rx_after.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!running.contains("s1"));

        // done を出さずに落ちたら、こちらで失敗として閉じる
        let bad = fake(&dir, "echo oops >&2; exit 3");
        let ev =
            collect(start(&running, job(&bad, "s2", &dir, String::new()), || {}).unwrap()).await;
        assert_eq!(ev[0]["ok"], false);
        assert!(ev[0]["text"].as_str().unwrap().contains("oops"));
        assert!(
            start(
                &running,
                job("/nonexistent/x", "s3", &dir, String::new()),
                || {}
            )
            .is_err()
        );
        assert!(!running.contains("s3"));
    }

    #[tokio::test]
    async fn stops_on_timeout_and_survives_noisy_stderr() {
        let dir = crate::db::temp_dir("send-timeout");
        let running = Running::default();
        let slow = fake(&dir, "sleep 30");
        let mut j = job(&slow, "t1", &dir, String::new());
        j.timeout = Duration::from_secs(1);
        let started = std::time::Instant::now();
        let ev = collect(start(&running, j, || {}).unwrap()).await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(ev[0]["ok"], false);
        assert!(ev[0]["text"].as_str().unwrap().starts_with("stopped after"));
        assert!(!running.contains("t1"));
        // パイプの容量（64KB）を超えるエラー出力を出しても止まらない
        let noisy = fake(
            &dir,
            r#"head -c 300000 /dev/zero | tr '\0' x >&2; echo '{"type":"result","subtype":"success","is_error":false}'"#,
        );
        let ev =
            collect(start(&running, job(&noisy, "t2", &dir, String::new()), || {}).unwrap()).await;
        assert_eq!(ev, [json!({"kind":"done","ok":true,"text":null})]);
    }

    #[tokio::test]
    async fn claims_the_new_session_id() {
        let dir = crate::db::temp_dir("send-newid");
        let running = Running::default();
        // 始めてすぐ ID を知らせ、少し動いてから終わる
        let script = fake(
            &dir,
            r#"echo '{"type":"system","subtype":"init","session_id":"real-1"}'; sleep 1; echo '{"type":"result","subtype":"success","is_error":false}'"#,
        );
        let mut rx = start(&running, job(&script, "new:/r", &dir, String::new()), || {}).unwrap();
        let first: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(first["kind"], "session");
        // 実行中は、できたセッションに画面から送れない
        assert!(running.contains("real-1"));
        collect(rx).await;
        assert!(!running.contains("real-1") && !running.contains("new:/r"));
    }

    #[test]
    fn remembered_rules() {
        let bash = |c: &str| rule_label(&remember_rule("Bash", &json!({ "command": c })));
        assert_eq!(bash("cargo test --release"), "Bash(cargo test:*)");
        assert_eq!(bash("npm run build"), "Bash(npm run:*)");
        assert_eq!(bash("git -C x log"), "Bash(git:*)"); // 2 語目がオプションなら 1 語
        assert_eq!(bash("python3 -c 'print(1)'"), "Bash(python3:*)");
        assert_eq!(bash("ls"), "Bash(ls:*)");
        assert_eq!(bash(""), "Bash");
        assert_eq!(
            rule_label(&remember_rule("WebFetch", &json!({ "url": "https://x" }))),
            "WebFetch"
        );
    }

    #[tokio::test]
    async fn relays_permission_requests() {
        let dir = crate::db::temp_dir("send-ask");
        let running = Running::default();
        // initialize と指示を読み、フックの問い合わせ（受けない）と許可の問い合わせを出し、答えを返答にして終わる
        let script = fake(
            &dir,
            r#"read init; read user
echo '{"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback"}}'
read hook
case "$hook" in *'"error"'*) ;; *) exit 9;; esac
echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","display_name":"Bash","input":{"command":"touch x"}}}'
read ans
case "$ans" in *'"localSettings"'*) case "$ans" in *'"touch:*"'*) t=remembered;; *) t=other;; esac;; *'"allow"'*'"touch x"'*) t=allowed;; *'"deny"'*) t=denied;; *) t=other;; esac
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s"}]}}\n' "$t"
echo '{"type":"result","subtype":"success","is_error":false}'
read rest || true"#,
        );
        for (allow, remember, want) in [
            (true, false, "allowed"),
            (false, false, "denied"),
            (true, true, "remembered"),
            (false, true, "denied"), // 拒否なら覚えない
        ] {
            let mut j = job(
                &script,
                "s-ask",
                &dir,
                "{\"init\":1}\n{\"user\":1}\n".into(),
            );
            j.ask = true;
            let mut rx = start(&running, j, || {}).unwrap();
            let ev: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
            assert_eq!(ev["kind"], "permission");
            assert_eq!(
                (ev["tool"].as_str(), ev["text"].as_str()),
                (Some("Bash"), Some("touch x"))
            );
            assert_eq!(ev["rule"], "Bash(touch:*)");
            // 画面を開き直したときは、そのセッションの待っている問い合わせを取り直せる
            assert_eq!(running.asks("s-ask"), std::slice::from_ref(&ev));
            assert!(running.asks("other").is_empty());
            running.answer("r1", allow, remember).unwrap();
            assert!(running.asks("s-ask").is_empty());
            // 答えたものには二度答えられない
            assert!(running.answer("r1", allow, false).is_err());
            let rest = collect(rx).await;
            assert_eq!(rest[0]["text"], want);
            assert_eq!(rest[1]["kind"], "done");
            assert_eq!(rest[1]["ok"], true);
        }
        assert!(running.answer("never", true, false).is_err());
        assert!(running.idle());
        let held = running.claim("busy").unwrap();
        assert!(!running.idle());
        // 動いているあいだは入れ替えを始めない
        assert!(!running.begin_update());
        drop(held);
        assert!(running.idle());
        // 入れ替えを始めたら、新しい実行は受け付けない。やめたら戻る
        assert!(running.begin_update());
        assert!(running.claim("new").is_none());
        let refused =
            start(&running, job(&script, "s-up", &dir, String::new()), || {}).unwrap_err();
        assert!(refused.to_string().contains("updating"));
        running.end_update();
        assert!(running.claim("new").is_some());
    }

    #[tokio::test]
    async fn removes_attached_files() {
        let dir = crate::db::temp_dir("send-cleanup");
        let running = Running::default();
        let file = |name: &str| {
            let f = dir.join(name);
            std::fs::write(&f, b"x").unwrap();
            f
        };
        // 終わったら消す
        let ok = fake(
            &dir,
            r#"echo '{"type":"result","subtype":"success","is_error":false}'"#,
        );
        let mut j = job(&ok, "c1", &dir, String::new());
        let done = file("done.png");
        j.cleanup = vec![done.clone()];
        collect(start(&running, j, || {}).unwrap()).await;
        assert!(!done.exists());
        // 起動できなかったときも消す
        let mut j = job("/nonexistent/x", "c2", &dir, String::new());
        let failed = file("failed.png");
        j.cleanup = vec![failed.clone()];
        assert!(start(&running, j, || {}).is_err());
        assert!(!failed.exists());
        // 同じセッションが実行中で断ったときも消す
        let _busy = running.claim("c3").unwrap();
        let mut j = job(&ok, "c3", &dir, String::new());
        let refused = file("refused.png");
        j.cleanup = vec![refused.clone()];
        assert!(start(&running, j, || {}).is_err());
        assert!(!refused.exists());
    }
}
