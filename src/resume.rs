use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// `claude agents --json` の 1 行。実行中のセッションの一覧
#[derive(Debug, Deserialize, PartialEq)]
pub struct Agent {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub kind: String, // background / interactive
}

pub fn agents() -> Result<Vec<Agent>> {
    let out = Command::new("claude")
        .args(["agents", "--json"])
        .stdin(Stdio::null())
        .output()
        .context("cannot start claude")?;
    if !out.status.success() {
        bail!(
            "claude agents failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).context("cannot parse claude agents output")
}

#[derive(Debug, PartialEq)]
pub enum Plan {
    /// 端末で開いている。裏で再開すると別のコピーができるので断る
    Refuse,
    /// すでに裏で動いている。起動はせず、つなぎ先だけ返す
    Running,
    /// 裏で起動する。まず respawn（一度裏に出したことのあるセッションを同じ ID で戻す）を試し、
    /// 知らないと言われたら Remote Control を付けて --bg --resume する。
    /// 逆順にすると、裏に出したことのあるセッションで別 ID のコピーができる（実測）
    Start,
}

pub fn plan(agents: &[Agent], id: &str) -> Plan {
    match agents
        .iter()
        .find(|a| a.session_id == id)
        .map(|a| a.kind.as_str())
    {
        Some("background") => Plan::Running,
        Some(_) => Plan::Refuse,
        None => Plan::Start,
    }
}

/// 再開する場所。記録の作業ディレクトリが消えていればリポジトリの本体で
pub fn workdir<'a>(cwd: &'a str, repo: &'a str) -> Option<&'a str> {
    [cwd, repo]
        .into_iter()
        .find(|d| !d.is_empty() && Path::new(d).is_dir())
}

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:@".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// 手で続きを打つときのコマンド
pub fn resume_command(source: &str, cwd: &str, id: &str) -> String {
    let cli = if source == "codex" {
        "codex resume"
    } else {
        "claude --resume"
    };
    format!("cd {} && {cli} {id}", shell_quote(cwd))
}

pub fn claude_args(id: &str, first: bool, title: &str) -> Vec<String> {
    if !first {
        return vec!["respawn".into(), short(id).into()];
    }
    let mut args = vec![
        "--bg".to_string(),
        "--resume".into(),
        id.into(),
        "--remote-control".into(),
    ];
    if !title.trim().is_empty() {
        args.push(title.chars().take(60).collect());
    }
    args
}

/// claude --bg が使う短い ID（セッション ID の先頭 8 文字）
fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// ログから Remote Control のつなぎ先を拾う
pub fn find_url(log: &str) -> Option<String> {
    let start = log.rfind("https://claude.ai/code/session_")?;
    let url: String = log[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || "/:._-".contains(*c))
        .collect();
    Some(url)
}

/// 画面の「Remote Control で再開」。ここから下は claude と systemd を実際に動かすので単体テストの外
pub fn resume(conn: &Connection, id: &str) -> Result<Value> {
    let Some((cwd, repo, title, file, source, machine)) = conn
        .query_row(
            "SELECT cwd, repo, title, file, source, machine FROM sessions WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
    else {
        bail!("no such session");
    };
    if !machine.is_empty() {
        bail!("this session ran on {machine}; resume it there (copy the resume command)");
    }
    if source != "claude" {
        bail!(
            "Remote Control resume is only for Claude Code sessions; copy the resume command instead"
        );
    }
    match plan(&agents()?, id) {
        Plan::Refuse => {
            bail!("this session is open in a terminal; continue there, or close it first")
        }
        Plan::Running => Ok(json!({ "status": "running", "url": wait_url(short(id), 1) })),
        Plan::Start => {
            let Some(dir) = workdir(&cwd, &repo) else {
                bail!("neither the working directory {cwd} nor the repository {repo} exists");
            };
            if spawn(dir, &claude_args(id, false, &title)).is_err() {
                // 元の記録が消えたセッションに --resume を打つと、claude は黙って空の新しいセッションを作る
                if !Path::new(&file).exists() {
                    bail!(
                        "the transcript {file} is gone (Claude Code cleans up old transcripts), so it cannot be resumed"
                    );
                }
                spawn(dir, &claude_args(id, true, &title))?;
            }
            Ok(json!({ "status": "started", "url": wait_url(short(id), 10) }))
        }
    }
}

/// claude --bg は裏の常駐役を立ち上げることがある。agent-calendar.service の中で立つと
/// agent-calendar を再起動したときに巻き込まれるので、systemd があれば別の scope に出す。
/// systemd の無い環境（macOS など）では、そのまま起動して終わるのを待つ
fn spawn(dir: &str, args: &[String]) -> Result<()> {
    let unit = format!(
        "agent-calendar-resume-{}",
        chrono::Utc::now().timestamp_millis()
    );
    let mut cmd = if has_systemd_run() {
        let mut c = Command::new("systemd-run");
        c.args([
            "--user",
            "--scope",
            "--collect",
            "--quiet",
            "--unit",
            &unit,
            "claude",
        ]);
        c
    } else {
        Command::new("claude")
    };
    let out = cmd
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .context("cannot start claude")?;
    if !out.status.success() {
        bail!(
            "resume failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn has_systemd_run() -> bool {
    Command::new("systemd-run")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Remote Control のつなぎ先がログに出るまで少し待つ。出なければ None
/// （claude.ai の Code の一覧にはセッションの題で出るので、そちらから開ける）
fn wait_url(short: &str, tries: usize) -> Option<String> {
    for i in 0..tries {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(1));
        }
        let out = Command::new("claude")
            .args(["logs", short])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if let Some(url) = find_url(&String::from_utf8_lossy(&out.stdout)) {
            return Some(url);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, kind: &str) -> Agent {
        Agent {
            session_id: id.into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn plans() {
        let list = [agent("a", "interactive"), agent("b", "background")];
        assert_eq!(plan(&list, "a"), Plan::Refuse);
        assert_eq!(plan(&list, "b"), Plan::Running);
        assert_eq!(plan(&list, "c"), Plan::Start);
    }

    #[test]
    fn parses_agents_json() {
        let v: Vec<Agent> = serde_json::from_str(
            r#"[{"id":"aff80c93","sessionId":"s1","kind":"background","state":"blocked"},{"pid":1,"sessionId":"s2","kind":"interactive"}]"#,
        )
        .unwrap();
        assert_eq!(v, [agent("s1", "background"), agent("s2", "interactive")]);
    }

    #[test]
    fn respawn_or_first_resume_args() {
        assert_eq!(
            claude_args("x", true, "題"),
            ["--bg", "--resume", "x", "--remote-control", "題"]
        );
        assert_eq!(
            claude_args("x", true, " "),
            ["--bg", "--resume", "x", "--remote-control"]
        );
        assert_eq!(
            claude_args("ee881feb-62e2", false, "題"),
            ["respawn", "ee881feb"]
        );
        assert_eq!(
            claude_args("x", true, &"あ".repeat(100))[4].chars().count(),
            60
        );
    }

    #[test]
    fn quoting_and_command() {
        assert_eq!(shell_quote("/home/u/acme-api"), "/home/u/acme-api");
        assert_eq!(shell_quote("/mnt/d/my dir"), "'/mnt/d/my dir'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(
            resume_command("claude", "/r/a b", "id1"),
            "cd '/r/a b' && claude --resume id1"
        );
        assert_eq!(
            resume_command("codex", "/r/a", "id2"),
            "cd /r/a && codex resume id2"
        );
    }

    #[test]
    fn workdir_falls_back_to_repo() {
        let tmp = std::env::temp_dir();
        let t = tmp.to_str().unwrap();
        assert_eq!(workdir(t, "/nonexistent"), Some(t));
        assert_eq!(workdir("/nonexistent", t), Some(t));
        assert_eq!(workdir("", "/nonexistent"), None);
    }

    #[test]
    fn finds_url_in_ansi_log() {
        let log = "\x1b[2C/remote-control is active\x1b[38;2;153m · Continue here, on your phone, or at https://claude.ai/code/session_01ExampleExampleExample0\x1b[39m";
        assert_eq!(
            find_url(log).as_deref(),
            Some("https://claude.ai/code/session_01ExampleExampleExample0")
        );
        assert_eq!(find_url("nothing"), None);
    }
}
