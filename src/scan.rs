use crate::pricing::Tokens;
use crate::remote::{self, Remote};
use crate::repo::{self, Resolver};
use crate::{codex, db};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const BUCKET_SECS: i64 = 600;

/// 1 ファイル = 1 セッション。サブエージェントは {session}/subagents/ の下にあり、
/// トークン数だけを親のセッションに足す（帯の濃さや依頼の一覧には入れない）
fn session_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    // Claude Code を使っていない環境もある
    if !root.is_dir() {
        return Ok(files);
    }
    for dir in std::fs::read_dir(root).with_context(|| format!("cannot read {}", root.display()))? {
        let dir = dir?.path();
        if !dir.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(&dir)? {
            let f = f?.path();
            if f.extension().is_some_and(|e| e == "jsonl") {
                files.push(f);
            }
        }
    }
    Ok(files)
}

/// 記録の置き場所
pub struct Roots {
    pub claude: PathBuf,
    pub codex: PathBuf,
    /// 手元は空文字。--remote で写したものは ssh のホスト名
    pub machine: String,
}

impl Roots {
    pub fn home() -> Self {
        let home = db::home();
        Roots {
            claude: home.join(".claude/projects"),
            codex: home.join(".codex/sessions"),
            machine: String::new(),
        }
    }

    pub fn remote(r: &Remote) -> Self {
        Roots {
            claude: r.claude_dir(),
            codex: r.codex_dir(),
            machine: r.name.clone(),
        }
    }
}

/// 変わったファイルだけ読み直す。戻り値は読み直したセッション数。
/// 別のマシンの分は、先に `remote::pull_all` で写しておく
pub fn run(conn: &mut Connection) -> Result<usize> {
    let mut n = run_at(conn, &Roots::home())?;
    for r in remote::list(conn)? {
        n += run_at(conn, &Roots::remote(&r))?;
    }
    Ok(n)
}

/// 読んだ記録の印（大きさ・更新時刻）と、セッションにしたもの（依頼が無ければ None）
type Read = ((i64, i64), Option<Parsed>);

/// 読めなかった記録があっても、ほかの記録は読み続ける（消えた・書きかけ・権限の無いもの）。
/// 読めなかった数と最初の理由は `job_status` の scan に残して画面に出す
fn run_at(conn: &mut Connection, roots: &Roots) -> Result<usize> {
    let mut resolver = Resolver::default();
    let (mut n, mut failed) = (0, Vec::new());
    for file in session_files(&roots.claude)? {
        let subagents = subagent_files(&file);
        let read_one = |conn: &Connection| -> Result<Option<Read>> {
            let fp = fingerprint(std::iter::once(&file).chain(&subagents))?;
            if unchanged(conn, &file, fp)? {
                return Ok(None);
            }
            let Some(mut p) = parse(&read(&file)?) else {
                return Ok(Some((fp, None)));
            };
            for sub in &subagents {
                add_usage(&mut p.usage, &read(sub)?, Kind::Subagent);
            }
            Ok(Some((fp, Some(p))))
        };
        match read_one(conn) {
            Ok(Some((fp, Some(p)))) => {
                let repo = repo_of(conn, &mut resolver, roots, &p)?;
                store(conn, &file.to_string_lossy(), fp, &repo, &roots.machine, &p)?;
                n += 1;
            }
            Ok(Some((fp, None))) => skip(conn, &file, fp)?,
            Ok(None) => {}
            Err(e) => failed.push(format!("{e:#}")),
        }
    }
    for file in codex::session_files(&roots.codex)? {
        let read_one = |conn: &Connection| -> Result<Option<Read>> {
            let fp = fingerprint(std::iter::once(&file))?;
            if unchanged(conn, &file, fp)? {
                return Ok(None);
            }
            Ok(Some((fp, codex::parse(&read(&file)?))))
        };
        match read_one(conn) {
            Ok(Some((fp, Some(p)))) => {
                let repo = repo_of(conn, &mut resolver, roots, &p)?;
                store(conn, &file.to_string_lossy(), fp, &repo, &roots.machine, &p)?;
                n += 1;
            }
            Ok(Some((fp, None))) => skip(conn, &file, fp)?,
            Ok(None) => {}
            Err(e) => failed.push(format!("{e:#}")),
        }
    }
    if let Some(first) = failed.first() {
        eprintln!("[scan] could not read {} files: {first}", failed.len());
    }
    let error = failed
        .first()
        .map(|first| format!("could not read {} files: {first}", failed.len()));
    crate::status::set(
        conn,
        &format!("scan{}", machine_suffix(&roots.machine)),
        error.as_deref(),
    )?;
    Ok(n)
}

fn machine_suffix(machine: &str) -> String {
    if machine.is_empty() {
        String::new()
    } else {
        format!(":{machine}")
    }
}

/// セッションにしなかった記録の印を残す（変わらなければ次から読まない）
fn skip(conn: &Connection, file: &Path, (size, mtime): (i64, i64)) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO skipped_files (file, file_size, file_mtime) VALUES (?1, ?2, ?3)",
        params![file.to_string_lossy(), size, mtime],
    )?;
    Ok(())
}

/// 手元のセッションは git に聞いて本体に寄せる。別のマシンのものは手元に無いので、パスだけで寄せる。
/// 作業場所（worktree）が消えていて寄せ先が分からないときは、前に読んだときに寄せた先を使う
/// （agent-calendar が切った worktree は置き場所の名前からは本体が分からない）
fn repo_of(
    conn: &Connection,
    resolver: &mut Resolver,
    roots: &Roots,
    p: &Parsed,
) -> Result<String> {
    let repo = if roots.machine.is_empty() {
        resolver.resolve(&p.cwd)
    } else {
        repo::without_worktree(&p.cwd)
    };
    // 別のマシンのセッションはパスだけで寄せている（手元のフォルダがあるかは関係ない）
    if !roots.machine.is_empty() || repo != p.cwd || Path::new(&p.cwd).is_dir() {
        return Ok(repo);
    }
    Ok(conn
        .query_row(
            "SELECT repo FROM sessions WHERE (id = ?1 OR cwd = ?2) AND machine = '' AND repo != cwd LIMIT 1",
            params![p.id, p.cwd],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(repo))
}

fn read(file: &Path) -> Result<String> {
    std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))
}

/// 前に読んだときと大きさ・更新時刻が同じか（セッションにしたものも、しなかったものも）
fn unchanged(conn: &Connection, file: &Path, (size, mtime): (i64, i64)) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sessions WHERE file = ?1 AND file_size = ?2 AND file_mtime = ?3)
             OR EXISTS (SELECT 1 FROM skipped_files WHERE file = ?1 AND file_size = ?2 AND file_mtime = ?3)",
        params![file.to_string_lossy(), size, mtime],
        |r| r.get(0),
    )?)
}

fn subagent_files(session_file: &Path) -> Vec<PathBuf> {
    let dir = session_file.with_extension("").join("subagents");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|f| f.extension().is_some_and(|e| e == "jsonl"))
        .collect()
}

/// 本体とサブエージェントの記録をまとめた変更の印（大きさの合計・一番新しい更新時刻）
fn fingerprint<'a>(files: impl Iterator<Item = &'a PathBuf>) -> Result<(i64, i64)> {
    let (mut size, mut mtime) = (0, 0);
    for f in files {
        let meta = std::fs::metadata(f)?;
        size += meta.len() as i64;
        mtime = mtime.max(meta.modified()?.duration_since(UNIX_EPOCH)?.as_secs() as i64);
    }
    Ok((size, mtime))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Main,
    Advisor,
    Subagent,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Main => "main",
            Kind::Advisor => "advisor",
            Kind::Subagent => "subagent",
        }
    }
}

/// (10 分の枠, モデル, 種類, fast mode か)
pub type UsageKey = (i64, String, Kind, bool);

#[derive(Debug, Default)]
pub struct Parsed {
    /// claude / codex
    pub source: &'static str,
    pub id: String,
    pub cwd: String,
    pub branch: String,
    pub title: String,
    pub first_ts: i64,
    pub last_ts: i64,
    pub last_uuid: String,
    pub buckets: BTreeMap<i64, i64>,
    pub usage: BTreeMap<UsageKey, Tokens>,
    pub prompts: Vec<(i64, String)>,
    /// エージェントの返答の文章（検索用。1 つずつ REPLY_CHARS で切る）
    pub replies: Vec<String>,
    /// エージェントの返答で終わっている（ツールを動かしている途中ではなく、こちらの次の指示を待っている）
    pub waiting: bool,
}

/// 検索に入れる返答 1 つの長さの上限（長い出力で DB が膨らまないように）
pub const REPLY_CHARS: usize = 2000;

pub fn clip_reply(t: &str) -> String {
    t.trim().chars().take(REPLY_CHARS).collect()
}

/// 発言が 1 つも無いファイル（開いてすぐ閉じたもの）は None
pub fn parse(text: &str) -> Option<Parsed> {
    let mut p = Parsed {
        source: "claude",
        ..Default::default()
    };
    // 1 応答が content ブロックごとに複数行へ分かれ、usage が重複して載る
    let mut seen_msg = HashSet::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = v["type"].as_str().unwrap_or("");
        if kind == "ai-title" {
            if let Some(t) = v["aiTitle"].as_str() {
                p.title = t.to_string();
            }
            continue;
        }
        if kind != "user" && kind != "assistant" || v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let Some(ts) = v["timestamp"].as_str().and_then(parse_ts) else {
            continue;
        };
        if p.id.is_empty() {
            p.id = v["sessionId"].as_str().unwrap_or("").to_string();
            p.first_ts = ts;
        }
        if p.cwd.is_empty() {
            p.cwd = v["cwd"].as_str().unwrap_or("").to_string();
            p.branch = v["gitBranch"].as_str().unwrap_or("").to_string();
        }
        p.last_ts = p.last_ts.max(ts);
        p.last_uuid = v["uuid"].as_str().unwrap_or("").to_string();
        *p.buckets
            .entry(ts / 1000 / BUCKET_SECS * BUCKET_SECS)
            .or_default() += 1;

        if kind == "user" {
            if let Some(t) = prompt_text(&v) {
                p.prompts.push((ts, t));
            }
            p.waiting = false;
            continue;
        }
        if seen_msg.insert(v["message"]["id"].as_str().unwrap_or("").to_string()) {
            usage_rows(&v, ts, Kind::Main, &mut p.usage);
        }
        let blocks = v["message"]["content"].as_array();
        p.waiting = blocks.is_some_and(|b| !b.iter().any(|x| x["type"] == "tool_use"));
        for t in blocks
            .into_iter()
            .flatten()
            .filter(|x| x["type"] == "text")
            .filter_map(|x| x["text"].as_str())
            .filter(|t| !t.trim().is_empty())
        {
            p.replies.push(clip_reply(t));
        }
    }
    if p.id.is_empty() || p.prompts.is_empty() {
        return None;
    }
    Some(p)
}

pub fn parse_ts(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// サブエージェントの記録からトークン数だけを拾う
fn add_usage(usage: &mut BTreeMap<UsageKey, Tokens>, text: &str, kind: Kind) {
    let mut seen_msg = HashSet::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["type"] != "assistant" {
            continue;
        }
        let Some(ts) = v["timestamp"].as_str().and_then(parse_ts) else {
            continue;
        };
        if seen_msg.insert(v["message"]["id"].as_str().unwrap_or("").to_string()) {
            usage_rows(&v, ts, kind, usage);
        }
    }
}

/// 1 応答の usage。上位の usage は本体の分だけで、advisor の呼び出しは
/// `iterations` に別モデルの行として入っている
fn usage_rows(v: &Value, ts: i64, kind: Kind, out: &mut BTreeMap<UsageKey, Tokens>) {
    let msg = &v["message"];
    let bucket = ts / 1000 / BUCKET_SECS * BUCKET_SECS;
    let u = &msg["usage"];
    if let Some(model) = msg["model"].as_str().filter(|m| !m.starts_with('<')) {
        let fast = u["speed"] == "fast";
        out.entry((bucket, model.to_string(), kind, fast))
            .or_default()
            .add(&tokens(u));
    }
    for it in u["iterations"].as_array().into_iter().flatten() {
        if it["type"] != "advisor_message" {
            continue;
        }
        let Some(model) = it["model"].as_str() else {
            continue;
        };
        out.entry((bucket, model.to_string(), Kind::Advisor, false))
            .or_default()
            .add(&tokens(it));
    }
}

/// キャッシュ書き込みの保持時間の内訳が無い古い記録は、1 時間保持として数える
/// （Claude Code は 1 時間保持で書いている）
fn tokens(u: &Value) -> Tokens {
    let n = |k: &str| u[k].as_i64().unwrap_or(0);
    let cc = &u["cache_creation"];
    let (c5, c1) = if cc.is_object() {
        (
            cc["ephemeral_5m_input_tokens"].as_i64().unwrap_or(0),
            cc["ephemeral_1h_input_tokens"].as_i64().unwrap_or(0),
        )
    } else {
        (0, n("cache_creation_input_tokens"))
    };
    Tokens {
        input: n("input_tokens"),
        output: n("output_tokens"),
        cache_read: n("cache_read_input_tokens"),
        cache_5m: c5,
        cache_1h: c1,
    }
}

/// 人が打った依頼の本文。ツール結果・自動で差し込まれる文・画像の注記は除く。
/// スラッシュコマンドは `/name 引数` の形に戻す
pub fn prompt_text(v: &Value) -> Option<String> {
    if v["isMeta"].as_bool() == Some(true) {
        return None;
    }
    // サブエージェントの報告・タスク完了の通知も user として入る。人が打ったものだけ残す
    if v["origin"]["kind"].as_str().is_some_and(|k| k != "human") {
        return None;
    }
    let content = &v["message"]["content"];
    let raw = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    if let Some(cmd) = tag(&raw, "command-name") {
        let args = tag(&raw, "command-args").unwrap_or_default();
        return Some(format!("{cmd} {args}").trim().to_string());
    }
    // origin を持たない古い記録では、自動で差し込まれた文・別セッションからのメッセージを書き出しで見分ける。
    // 人が打ったと記録にあるもの（貼り付けだけの依頼は `<pasted_content …>` で始まる）は残す
    let human = v["origin"]["kind"].as_str() == Some("human");
    if !human
        && (raw.trim_start().starts_with('<')
            || raw.starts_with("Caveat:")
            || raw.starts_with("Another Claude session sent a message"))
    {
        return None;
    }
    let text = raw
        .lines()
        .filter(|l| !l.starts_with("[Image"))
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn tag(s: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close)? + start;
    Some(s[start..end].trim().to_string())
}

fn store(
    conn: &mut Connection,
    file: &str,
    (size, mtime): (i64, i64),
    repo: &str,
    machine: &str,
    p: &Parsed,
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM skipped_files WHERE file = ?1", [file])?;
    for t in ["activity", "prompts", "usage", "texts"] {
        tx.execute(&format!("DELETE FROM {t} WHERE session_id = ?1"), [&p.id])?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO sessions
         (id, file, file_size, file_mtime, repo, cwd, branch, title, first_ts, last_ts, last_uuid, prompt_count, source, machine, waiting)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        params![
            p.id, file, size, mtime, repo, p.cwd, p.branch, p.title, p.first_ts, p.last_ts,
            p.last_uuid, p.prompts.len() as i64, p.source, machine, p.waiting,
        ],
    )?;
    for text in p.prompts.iter().map(|(_, t)| t).chain(&p.replies) {
        tx.execute(
            "INSERT INTO texts (session_id, text) VALUES (?1, ?2)",
            params![p.id, text],
        )?;
    }
    for (bucket, n) in &p.buckets {
        tx.execute(
            "INSERT INTO activity (session_id, bucket, n) VALUES (?1, ?2, ?3)",
            params![p.id, bucket, n],
        )?;
    }
    for ((bucket, model, kind, fast), t) in &p.usage {
        tx.execute(
            "INSERT INTO usage VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                p.id,
                bucket,
                model,
                kind.as_str(),
                fast,
                t.input,
                t.output,
                t.cache_read,
                t.cache_5m,
                t.cache_1h
            ],
        )?;
    }
    for (i, (ts, text)) in p.prompts.iter().enumerate() {
        tx.execute(
            "INSERT INTO prompts (session_id, seq, ts, text) VALUES (?1, ?2, ?3, ?4)",
            params![p.id, i as i64, ts, text],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"type":"mode","mode":"normal","sessionId":"s1"}
{"type":"user","sessionId":"s1","uuid":"u1","timestamp":"2026-10-02T21:15:43.294Z","cwd":"/r/a","gitBranch":"main","message":{"content":"依存を最新にして"}}
{"type":"assistant","sessionId":"s1","uuid":"u2","timestamp":"2026-10-02T21:15:45.000Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":100,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}
{"type":"assistant","sessionId":"s1","uuid":"u3","timestamp":"2026-10-02T21:15:46.000Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":100,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}
{"type":"user","sessionId":"s1","uuid":"u4","timestamp":"2026-10-02T21:16:00.000Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}
{"type":"user","sessionId":"s1","uuid":"x","isSidechain":true,"timestamp":"2026-10-02T23:00:00.000Z","message":{"content":"サブ"}}
{"type":"user","sessionId":"s1","uuid":"u5","timestamp":"2026-10-02T21:40:00.000Z","message":{"content":"<command-name>/simplify</command-name><command-args>src</command-args>"}}
{"type":"ai-title","aiTitle":"依存の更新","sessionId":"s1"}
not json
"#;

    #[test]
    fn parses_session() {
        let p = parse(SAMPLE).unwrap();
        assert_eq!(p.id, "s1");
        assert_eq!(p.cwd, "/r/a");
        assert_eq!(p.title, "依存の更新");
        assert_eq!(p.last_uuid, "u5");
        // 同じ message.id の usage は 1 回だけ数える。内訳の無い書き込みは 1 時間保持
        let key = (
            sec("2026-10-02T21:10:00Z"),
            "claude-opus-5-5".to_string(),
            Kind::Main,
            false,
        );
        assert_eq!(
            p.usage[&key],
            Tokens {
                input: 2,
                output: 100,
                cache_read: 10,
                cache_5m: 0,
                cache_1h: 5
            }
        );
        assert_eq!(
            p.prompts
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>(),
            ["依存を最新にして", "/simplify src"]
        );
        // サイドチェーンは数えない（23:00 の枠ができない）
        assert_eq!(p.buckets.len(), 2);
        assert_eq!(p.buckets.values().sum::<i64>(), 5);
    }

    fn sec(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp()
    }

    #[test]
    fn advisor_and_fast_usage() {
        let line = r#"{"type":"assistant","timestamp":"2026-10-02T21:15:45.000Z","message":{"id":"m","model":"claude-opus-5-5","usage":{"speed":"fast","input_tokens":4,"output_tokens":300,"cache_read_input_tokens":190,"cache_creation_input_tokens":10,"cache_creation":{"ephemeral_5m_input_tokens":3,"ephemeral_1h_input_tokens":7},"iterations":[{"type":"message","input_tokens":2},{"type":"advisor_message","model":"claude-fable-5-1","input_tokens":1000,"output_tokens":50},{"type":"advisor_message"}]}}}"#;
        let mut u = BTreeMap::new();
        add_usage(
            &mut u,
            &format!("{line}\n{line}\n{{\"type\":\"user\"}}\n{{\"type\":\"assistant\"}}"),
            Kind::Subagent,
        );
        let b = sec("2026-10-02T21:10:00Z");
        assert_eq!(u.len(), 2);
        assert_eq!(
            u[&(b, "claude-opus-5-5".into(), Kind::Subagent, true)],
            Tokens {
                input: 4,
                output: 300,
                cache_read: 190,
                cache_5m: 3,
                cache_1h: 7
            }
        );
        assert_eq!(
            u[&(b, "claude-fable-5-1".into(), Kind::Advisor, false)],
            Tokens {
                input: 1000,
                output: 50,
                ..Default::default()
            }
        );
        assert_eq!(Kind::Main.as_str(), "main");
    }

    #[test]
    fn run_reads_changed_files_only() {
        let dir = db::temp_dir("scan");
        let proj = dir.join("projects/-r-a");
        std::fs::create_dir_all(proj.join("s1/subagents")).unwrap();
        std::fs::write(proj.join("s1.jsonl"), SAMPLE).unwrap();
        std::fs::write(
            proj.join("s1/subagents/agent-x.jsonl"),
            SAMPLE.replace("s1", "sub"),
        )
        .unwrap();
        std::fs::write(proj.join("s1/subagents/notes.txt"), "").unwrap();
        std::fs::write(proj.join("empty.jsonl"), "{}").unwrap();
        std::fs::write(dir.join("projects/stray.txt"), "").unwrap();
        let mut conn = db::open_at(&dir.join("t.db")).unwrap();
        let roots = Roots {
            claude: dir.join("projects"),
            codex: dir.join("codex"),
            machine: String::new(),
        };
        assert_eq!(run_at(&mut conn, &roots).unwrap(), 1);
        assert_eq!(run_at(&mut conn, &roots).unwrap(), 0);
        let (repo, prompts, buckets): (String, i64, i64) = conn
            .query_row(
                "SELECT repo, (SELECT COUNT(*) FROM prompts), (SELECT COUNT(*) FROM activity) FROM sessions",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((repo.as_str(), prompts, buckets), ("/r/a", 2, 2));
        // サブエージェントはトークン数だけ親に入る
        let kinds: Vec<String> = conn
            .prepare("SELECT kind FROM usage ORDER BY kind")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(kinds, ["main", "subagent"]);
        // サブエージェントの記録だけが増えても読み直す
        std::fs::write(proj.join("s1/subagents/agent-y.jsonl"), SAMPLE).unwrap();
        assert_eq!(run_at(&mut conn, &roots).unwrap(), 1);
        // 追記されたら読み直し、古い行は入れ替わる
        let more = format!(
            "{SAMPLE}{}\n",
            r#"{"type":"user","sessionId":"s1","uuid":"u9","timestamp":"2026-10-03T01:00:00.000Z","message":{"content":"続き"}}"#
        );
        std::fs::write(proj.join("s1.jsonl"), more).unwrap();
        assert_eq!(run_at(&mut conn, &roots).unwrap(), 1);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
        // 依頼の無い記録（empty.jsonl）は印だけ残し、変わらなければ読まない
        let skipped: i64 = conn
            .query_row("SELECT COUNT(*) FROM skipped_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(skipped, 1);
        // 読めない記録（UTF-8 でない）があっても、ほかは読み続け、数と理由を残す
        std::fs::write(proj.join("broken.jsonl"), [0xff, 0xfe, 0x00]).unwrap();
        let more = format!(
            "{SAMPLE}{}\n",
            r#"{"type":"user","sessionId":"s1","uuid":"u10","timestamp":"2026-10-03T02:00:00.000Z","message":{"content":"さらに続き"}}"#
        );
        std::fs::write(proj.join("s1.jsonl"), more).unwrap();
        assert_eq!(run_at(&mut conn, &roots).unwrap(), 1);
        let st = crate::status::all(&conn).unwrap();
        assert_eq!(st[0]["name"], "scan");
        assert!(
            st[0]["error"]
                .as_str()
                .unwrap()
                .starts_with("could not read 1 files")
        );
        std::fs::remove_file(proj.join("broken.jsonl")).unwrap();
        run_at(&mut conn, &roots).unwrap();
        assert!(crate::status::all(&conn).unwrap()[0]["error"].is_null());
        // 依頼も返答も検索用に入る
        let texts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM texts WHERE text LIKE '%さらに%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(texts, 1);
    }

    #[test]
    fn keeps_the_repo_of_a_removed_worktree() {
        let dir = db::temp_dir("scan-gone");
        let proj = dir.join("projects/-x");
        std::fs::create_dir_all(&proj).unwrap();
        let gone = "/nonexistent/agent-calendar/worktrees/ab12-feature";
        let mut conn = db::open_at(&dir.join("t.db")).unwrap();
        // 前に読んだとき（worktree があったころ）は本体に寄せていた
        conn.execute(
            "INSERT INTO sessions VALUES ('s1', 'old', 0, 0, '/r/app', ?1, 'feature', '', 0, 0, 'u', 1, 'claude', '', 0)",
            [gone],
        )
        .unwrap();
        std::fs::write(proj.join("s1.jsonl"), SAMPLE.replace("/r/a", gone)).unwrap();
        let roots = Roots {
            claude: dir.join("projects"),
            codex: dir.join("codex"),
            machine: String::new(),
        };
        assert_eq!(run_at(&mut conn, &roots).unwrap(), 1);
        let repo: String = conn
            .query_row("SELECT repo FROM sessions WHERE id = 's1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(repo, "/r/app");
    }

    #[test]
    fn knows_when_the_agent_waits_for_the_next_instruction() {
        let user = r#"{"type":"user","sessionId":"w","uuid":"a","timestamp":"2026-10-02T21:00:00.000Z","cwd":"/r","message":{"content":"直して"}}"#;
        let tool = r#"{"type":"assistant","sessionId":"w","uuid":"b","timestamp":"2026-10-02T21:00:01.000Z","message":{"id":"m1","content":[{"type":"text","text":"見ます"},{"type":"tool_use","name":"Bash"}]}}"#;
        let result = r#"{"type":"user","sessionId":"w","uuid":"c","timestamp":"2026-10-02T21:00:02.000Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#;
        let reply = r#"{"type":"assistant","sessionId":"w","uuid":"d","timestamp":"2026-10-02T21:00:03.000Z","message":{"id":"m2","content":[{"type":"text","text":"直しました"}]}}"#;
        let p = parse(&[user, tool].join("\n")).unwrap();
        assert!(!p.waiting); // ツールを動かしている途中
        let p = parse(&[user, tool, result].join("\n")).unwrap();
        assert!(!p.waiting);
        let p = parse(&[user, tool, result, reply].join("\n")).unwrap();
        assert!(p.waiting);
        assert_eq!(p.replies, ["見ます", "直しました"]);
        assert_eq!(
            clip_reply(&"あ".repeat(REPLY_CHARS + 5)).chars().count(),
            REPLY_CHARS
        );
    }

    #[test]
    fn empty_session_is_none() {
        assert!(parse(r#"{"type":"mode","sessionId":"s"}"#).is_none());
    }

    #[test]
    fn prompt_filters() {
        let v = |c: Value| serde_json::json!({"type":"user","message":{"content":c}});
        assert_eq!(
            prompt_text(&v("<system-reminder>x</system-reminder>".into())),
            None
        );
        assert_eq!(
            prompt_text(&v("[Image: original 1x1]\nこれ見て".into())).as_deref(),
            Some("これ見て")
        );
        assert_eq!(prompt_text(&v("[Image: original 1x1]".into())), None);
        assert_eq!(
            prompt_text(&v(
                serde_json::json!([{"type":"image"},{"type":"text","text":"直して"}])
            ))
            .as_deref(),
            Some("直して")
        );
        let meta = serde_json::json!({"type":"user","isMeta":true,"message":{"content":"x"}});
        assert_eq!(prompt_text(&meta), None);
        let peer = serde_json::json!({"type":"user","origin":{"kind":"peer"},"message":{"content":"報告"}});
        assert_eq!(prompt_text(&peer), None);
        let human = serde_json::json!({"type":"user","origin":{"kind":"human"},"message":{"content":"直して"}});
        assert_eq!(prompt_text(&human).as_deref(), Some("直して"));
        // 貼り付けだけの依頼は < で始まるが、人が打ったものなので残す
        let pasted = serde_json::json!({"type":"user","origin":{"kind":"human"},"message":{"content":"<pasted_content id=\"a\">\n直して\n</pasted_content>"}});
        assert!(prompt_text(&pasted).unwrap().contains("直して"));
        assert_eq!(
            prompt_text(&v("Another Claude session sent a message: x".into())),
            None
        );
    }
}
