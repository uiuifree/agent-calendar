use crate::{codex, scan};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

pub const DEFAULT_MODEL: &str = "sonnet";
/// 動いている最中のセッションは要約しない（どうせ記録が増えて作り直しになる）
const IDLE_MS: i64 = 30 * 60 * 1000;
/// 1 ブロックあたり・全体の上限。超えた分は冒頭と末尾を残して間を落とす
const BLOCK_CHARS: usize = 800;
const TOTAL_CHARS: usize = 80_000;

/// 要約を書く言語
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lang {
    En,
    Ja,
}

impl Lang {
    /// 指定が無ければ $LC_ALL（無ければ $LANG）が ja で始まるとき日本語
    pub fn from_arg(arg: Option<&str>) -> Result<Self> {
        match arg {
            Some("en") => Ok(Lang::En),
            Some("ja") => Ok(Lang::Ja),
            Some(other) => bail!("unknown summary language: {other} (en or ja)"),
            None => Ok(Self::from_locale(
                &std::env::var("LC_ALL")
                    .ok()
                    .filter(|v| !v.is_empty())
                    .or_else(|| std::env::var("LANG").ok())
                    .unwrap_or_default(),
            )),
        }
    }

    fn from_locale(locale: &str) -> Self {
        if locale.starts_with("ja") {
            Lang::Ja
        } else {
            Lang::En
        }
    }

    fn instruction(self) -> &'static str {
        match self {
            Lang::En => {
                r#"stdin is the transcript of one AI coding agent session (the user's requests and the agent's replies, abridged).
Summarize what was done in this session and reply with this JSON only, no preface and no code fence:
{"title":"a short title, about 6 words","bullets":["one line per thing done or decided, 3 to 6 lines"],"status":"done | wip | talk","next":"what is left to do, or an empty string"}
status: done = the work was finished, wip = stopped partway, talk = only discussion or questions, nothing was changed.
Write plainly so the behavior and the outcome are clear. File names, commands and identifiers may be written as is."#
            }
            Lang::Ja => {
                r#"stdin は AI コーディングエージェントの 1 セッションの記録（ユーザーの依頼とエージェントの応答の抜粋）。
このセッションで何をしたかを日本語でまとめ、次の形の JSON だけを返す。前置き・コードフェンスは付けない。
{"title":"20字程度の題名","bullets":["やったこと・決めたことを1行ずつ、3〜6個"],"status":"done | wip | talk","next":"残っていること。無ければ空文字"}
status は done＝作業を終えた、wip＝途中で止まっている、talk＝相談や質問だけで何も変えていない。
挙動と結果がそのまま伝わる平易な言葉で書く。ファイル名・コマンド・識別子はそのまま書いてよい。"#
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub title: String,
    pub bullets: Vec<String>,
    pub status: String,
    #[serde(default)]
    pub next: String,
}

/// 状態を言語に依らない符号にそろえる。初期の要約は「完了／途中／相談のみ」で入っている
pub fn status_code(s: &str) -> &'static str {
    match s.trim() {
        "done" | "完了" => "done",
        "wip" | "途中" => "wip",
        "talk" | "相談のみ" => "talk",
        _ => "",
    }
}

/// 保存した要約を読む。状態は符号にそろえて返す
pub fn read(body: &str) -> Option<Summary> {
    let mut s: Summary = serde_json::from_str(body).ok()?;
    s.status = status_code(&s.status).to_string();
    Some(s)
}

/// 戻り値は要約できたセッション数。1 件の失敗では止めず、次へ進む
pub fn run(conn: &Connection, limit: usize, model: &str, lang: Lang) -> Result<usize> {
    run_with(conn, limit, model, lang, "claude")
}

fn run_with(
    conn: &Connection,
    limit: usize,
    model: &str,
    lang: Lang,
    claude: &str,
) -> Result<usize> {
    let now = chrono::Utc::now().timestamp_millis();
    let mut stmt = conn.prepare(
        "SELECT s.id, s.file, s.last_uuid, s.source FROM sessions s
         LEFT JOIN summaries m ON m.session_id = s.id
         WHERE s.last_ts < ?1 AND (m.session_id IS NULL OR m.last_uuid != s.last_uuid)
         ORDER BY s.last_ts DESC LIMIT ?2",
    )?;
    let todo: Vec<(String, String, String, String)> = stmt
        .query_map(params![now - IDLE_MS, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut done = 0;
    for (id, file, last_uuid, source) in todo {
        match make(conn, claude, (&id, &file, &last_uuid, &source), model, lang) {
            Ok(_) => done += 1,
            Err(e) => eprintln!("[summarize] {id} failed: {e:#}"),
        }
    }
    Ok(done)
}

/// 画面から押したセッションを、いま要約する（止まっていなくても、要約済みでも作り直す）
pub fn one(conn: &Connection, id: &str, model: &str, lang: Lang) -> Result<Summary> {
    one_with(conn, id, model, lang, "claude")
}

fn one_with(conn: &Connection, id: &str, model: &str, lang: Lang, claude: &str) -> Result<Summary> {
    let (file, last_uuid, source): (String, String, String) = conn
        .query_row(
            "SELECT file, last_uuid, source FROM sessions WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .with_context(|| format!("no such session: {id}"))?;
    make(conn, claude, (id, &file, &last_uuid, &source), model, lang)
}

/// 要約を作っている最中のセッション。裏の自動の要約と画面の「今すぐ要約」で同じセッションを
/// 二重に作らない（Claude を 2 回呼ぶうえ、古い内容で作ったほうが後から上書きしうる）
static BUSY: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// いま要約を作っているか（本体の入れ替えは、作り終わるまで待つ）
pub fn busy() -> bool {
    !BUSY.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

/// 印を付ける。すでに付いていれば None。戻り値を手放すと外れる（期間の要約も同じ印を使う）
pub fn claim(id: &str) -> Option<Busy> {
    let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    if busy.iter().any(|b| b == id) {
        return None;
    }
    busy.push(id.to_string());
    Some(Busy(id.to_string()))
}

pub struct Busy(String);

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|b| *b != self.0);
    }
}

/// 記録を読んで要約を作り、保存する。(id, file, last_uuid, source)
fn make(
    conn: &Connection,
    claude: &str,
    (id, file, last_uuid, source): (&str, &str, &str, &str),
    model: &str,
    lang: Lang,
) -> Result<Summary> {
    let Some(_busy) = claim(id) else {
        bail!("this session is already being summarized");
    };
    let text = std::fs::read_to_string(file).with_context(|| format!("cannot read {file}"))?;
    let (s, cost) = call_claude(claude, &digest(source, &text), model, lang)?;
    conn.execute(
        "INSERT OR REPLACE INTO summaries (session_id, last_uuid, model, body, created_at, cost_usd)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, last_uuid, model, serde_json::to_string(&s)?, chrono::Utc::now().timestamp_millis(), cost],
    )?;
    println!("[summarize] {id} {}", s.title);
    Ok(s)
}

/// 要約に渡す本文。依頼とエージェントの文章だけを時系列に並べる（ツールの入出力は渡さない）
pub fn digest(source: &str, text: &str) -> String {
    let blocks = if source == "codex" {
        codex::digest_blocks(text)
    } else {
        claude_blocks(text)
    };
    let lines: Vec<String> = blocks
        .iter()
        .map(|(user, t)| format!("[{}] {}", if *user { "user" } else { "agent" }, clip(t)))
        .collect();
    squeeze(&lines.join("\n\n"))
}

/// (人の依頼か, 本文)
fn claude_blocks(text: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        match v["type"].as_str() {
            Some("user") => {
                if let Some(t) = scan::prompt_text(&v) {
                    out.push((true, t));
                }
            }
            Some("assistant") => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    if let Some(t) = b["text"]
                        .as_str()
                        .filter(|t| b["type"] == "text" && !t.trim().is_empty())
                    {
                        out.push((false, t.to_string()));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn clip(s: &str) -> String {
    if s.chars().count() <= BLOCK_CHARS {
        return s.to_string();
    }
    format!("{}…", s.chars().take(BLOCK_CHARS).collect::<String>())
}

/// 長すぎるときは冒頭 1/4 と末尾 3/4 を残す（最後のほうに結論があることが多い）
fn squeeze(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= TOTAL_CHARS {
        return s.to_string();
    }
    let head = TOTAL_CHARS / 4;
    let tail = TOTAL_CHARS - head;
    format!(
        "{}\n\n[...]\n\n{}",
        chars[..head].iter().collect::<String>(),
        chars[chars.len() - tail..].iter().collect::<String>()
    )
}

fn call_claude(
    claude: &str,
    input: &str,
    model: &str,
    lang: Lang,
) -> Result<(Summary, Option<f64>)> {
    let (result, cost) = run_claude(claude, input, model, lang.instruction())?;
    let mut s = parse_summary(&result)?;
    s.status = status_code(&s.status).to_string();
    Ok((s, cost))
}

/// `claude -p` を記録を残さない設定で呼ぶ（残すと要約の呼び出し自体が日誌に載る）。返事の本文と金額
pub fn run_claude(
    claude: &str,
    input: &str,
    model: &str,
    instruction: &str,
) -> Result<(String, Option<f64>)> {
    let mut child = Command::new(claude)
        .args([
            "-p",
            "--no-session-persistence",
            "--model",
            model,
            "--tools",
            "",
            "--setting-sources",
            "",
            "--output-format",
            "json",
            instruction,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("cannot start claude")?;
    child
        .stdin
        .take()
        .context("no stdin")?
        .write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "claude failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let v: Value = serde_json::from_slice(&out.stdout).context("claude output is not JSON")?;
    let result = v["result"]
        .as_str()
        .context("claude output has no result")?;
    Ok((result.to_string(), v["total_cost_usd"].as_f64()))
}

/// 指示しても前置きやコードフェンスが付くことがあるので、最初の { から最後の } までを読む
pub fn parse_summary(s: &str) -> Result<Summary> {
    let (Some(a), Some(b)) = (s.find('{'), s.rfind('}')) else {
        bail!("no JSON in the summary: {s}");
    };
    serde_json::from_str(&s[a..=b]).with_context(|| format!("cannot parse the summary JSON: {s}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// claude の代わりに決まった JSON を返すスクリプト
    fn fake_claude(dir: &std::path::Path, stdout: &str, code: i32) -> String {
        let p = dir.join(format!("claude-{code}-{}", stdout.len()));
        crate::db::write_script(
            &p,
            &format!("#!/bin/sh\ncat >/dev/null\ncat <<'EOF'\n{stdout}\nEOF\nexit {code}\n"),
        );
        p.to_string_lossy().into_owned()
    }

    fn seed(conn: &Connection, dir: &std::path::Path, id: &str, last_ts: i64) {
        let file = dir.join(format!("{id}.jsonl"));
        std::fs::write(&file, r#"{"type":"user","message":{"content":"直して"}}"#).unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES (?1, ?2, 0, 0, '/r', '/r', 'main', '', 0, ?3, 'u1', 1, 'claude', '')",
            params![id, file.to_string_lossy(), last_ts],
        )
        .unwrap();
    }

    #[test]
    fn run_summarizes_idle_sessions_once() {
        let dir = crate::db::temp_dir("summarize");
        let conn = crate::db::open_at(&dir.join("t.db")).unwrap();
        seed(&conn, &dir, "old", 0);
        seed(&conn, &dir, "live", chrono::Utc::now().timestamp_millis());
        let ok = fake_claude(
            &dir,
            r#"{"result":"{\"title\":\"t\",\"bullets\":[],\"status\":\"完了\"}","total_cost_usd":0.03}"#,
            0,
        );
        assert_eq!(run_with(&conn, 10, "sonnet", Lang::Ja, &ok).unwrap(), 1);
        let (cost, body): (f64, String) = conn
            .query_row(
                "SELECT cost_usd, body FROM summaries WHERE session_id='old'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(cost, 0.03);
        // 状態は符号で保存する
        assert_eq!(read(&body).unwrap().status, "done");
        assert!(body.contains("\"done\""));
        // 要約済みで記録が増えていなければ飛ばす。増えたら作り直す
        assert_eq!(run_with(&conn, 10, "sonnet", Lang::En, &ok).unwrap(), 0);
        conn.execute("UPDATE sessions SET last_uuid = 'u2' WHERE id = 'old'", [])
            .unwrap();
        assert_eq!(run_with(&conn, 10, "sonnet", Lang::En, &ok).unwrap(), 1);
    }

    #[test]
    fn one_summarizes_on_demand() {
        let dir = crate::db::temp_dir("summarize-one");
        let conn = crate::db::open_at(&dir.join("t.db")).unwrap();
        // 止まっていない（いま動いている）セッションでも作る
        seed(&conn, &dir, "live", chrono::Utc::now().timestamp_millis());
        let ok = fake_claude(
            &dir,
            r#"{"result":"{\"title\":\"途中\",\"bullets\":[\"a\"],\"status\":\"途中\"}","total_cost_usd":0.01}"#,
            0,
        );
        let s = one_with(&conn, "live", "sonnet", Lang::Ja, &ok).unwrap();
        assert_eq!((s.title.as_str(), s.status.as_str()), ("途中", "wip"));
        let body: String = conn
            .query_row(
                "SELECT body FROM summaries WHERE session_id='live'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(read(&body).unwrap().bullets, ["a"]);
        // 要約済みでも作り直す
        assert!(one_with(&conn, "live", "sonnet", Lang::Ja, &ok).is_ok());
        // 作っている最中（裏の自動の要約など）なら断る。終われば外れる
        let held = claim("live").unwrap();
        assert!(busy());
        assert!(claim("live").is_none());
        assert!(one_with(&conn, "live", "sonnet", Lang::Ja, &ok).is_err());
        drop(held);
        assert!(one_with(&conn, "live", "sonnet", Lang::Ja, &ok).is_ok());
        assert!(one_with(&conn, "nope", "sonnet", Lang::Ja, &ok).is_err());
        assert!(
            one_with(
                &conn,
                "live",
                "sonnet",
                Lang::Ja,
                &fake_claude(&dir, "x", 1)
            )
            .is_err()
        );
    }

    #[test]
    fn run_skips_failures() {
        let dir = crate::db::temp_dir("summarize-fail");
        let conn = crate::db::open_at(&dir.join("t.db")).unwrap();
        seed(&conn, &dir, "a", 0);
        for bad in [
            fake_claude(&dir, "x", 1),
            fake_claude(&dir, "not json", 0),
            fake_claude(&dir, "{}", 0),
        ] {
            assert_eq!(run_with(&conn, 10, "sonnet", Lang::En, &bad).unwrap(), 0);
        }
        assert_eq!(
            run_with(&conn, 10, "sonnet", Lang::En, "/nonexistent/claude").unwrap(),
            0
        );
        conn.execute("UPDATE sessions SET file = '/nonexistent.jsonl'", [])
            .unwrap();
        assert_eq!(
            run_with(&conn, 10, "sonnet", Lang::En, &fake_claude(&dir, "{}", 0)).unwrap(),
            0
        );
    }

    #[test]
    fn digest_keeps_prompts_and_text_only() {
        let text = r#"{"type":"user","message":{"content":"直して"}}
{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"x"},{"type":"tool_use","name":"Bash"},{"type":"text","text":"直しました"}]}}
{"type":"user","message":{"content":[{"type":"tool_result","content":"log"}]}}
{"type":"user","isSidechain":true,"message":{"content":"サブ"}}"#;
        assert_eq!(
            digest("claude", text),
            "[user] 直して\n\n[agent] 直しました"
        );
        let codex = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix it"}]}}"#;
        assert_eq!(digest("codex", codex), "[user] fix it");
    }

    #[test]
    fn languages_and_status_codes() {
        assert_eq!(Lang::from_arg(Some("en")).unwrap(), Lang::En);
        assert_eq!(Lang::from_arg(Some("ja")).unwrap(), Lang::Ja);
        assert!(Lang::from_arg(Some("fr")).is_err());
        assert!(Lang::from_arg(None).is_ok());
        assert_eq!(Lang::from_locale("ja_JP.UTF-8"), Lang::Ja);
        assert_eq!(Lang::from_locale("C.UTF-8"), Lang::En);
        assert!(Lang::En.instruction().contains("done | wip | talk"));
        assert!(Lang::Ja.instruction().contains("done | wip | talk"));
        assert_eq!(status_code("途中"), "wip");
        assert_eq!(status_code("相談のみ"), "talk");
        assert_eq!(status_code(" done "), "done");
        assert_eq!(status_code("?"), "");
        assert!(read("not json").is_none());
    }

    #[test]
    fn squeeze_keeps_head_and_tail() {
        let s = "あ".repeat(TOTAL_CHARS) + &"い".repeat(10);
        let out = squeeze(&s);
        assert!(out.starts_with("あ"));
        assert!(out.ends_with("いいいいいいいいいい"));
        assert!(out.contains("[...]"));
        assert_eq!(squeeze("短い"), "短い");
    }

    #[test]
    fn clip_long_block() {
        assert_eq!(
            clip(&"x".repeat(BLOCK_CHARS + 5)).chars().count(),
            BLOCK_CHARS + 1
        );
        assert_eq!(clip("x"), "x");
    }

    #[test]
    fn parses_fenced_summary() {
        let s = "```json\n{\"title\":\"t\",\"bullets\":[\"a\"],\"status\":\"wip\"}\n```";
        assert_eq!(
            parse_summary(s).unwrap(),
            Summary {
                title: "t".into(),
                bullets: vec!["a".into()],
                status: "wip".into(),
                next: String::new()
            }
        );
        assert!(parse_summary("無理").is_err());
    }
}
