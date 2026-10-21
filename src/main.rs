//! agent-calendar — a local calendar for AI coding agent sessions (Claude Code and Codex).
//!
//! Reads the session transcripts that Claude Code and Codex write to your home directory,
//! keeps a SQLite index, and serves a calendar / stats page on 127.0.0.1.

mod codex;
mod db;
mod diff;
mod finished;
mod github;
mod limits;
mod manage;
mod period;
mod pins;
mod place;
mod pricing;
mod remote;
mod repo;
mod resume;
mod scan;
mod schedule;
mod search;
mod send;
mod serve;
mod service;
mod settings;
mod share;
mod stats;
mod status;
mod summarize;
mod transcript;
mod update;

use anyhow::{Result, bail};

const USAGE: &str = "\
agent-calendar — a local calendar for AI coding agent sessions (Claude Code, Codex)

USAGE:
    agent-calendar serve [--port 23848] [--no-summarize] [--summary-model sonnet] [--summary-lang en|ja]
        Serve the calendar on http://127.0.0.1:<port>/ . By default it rescans every 5 minutes and
        summarizes idle sessions every hour (10 per round), between 7:00 and 22:00.
        Change the hours and intervals from the settings in the page.
    agent-calendar scan
        Re-read changed transcripts into the index.
    agent-calendar summarize [--limit 20] [--model sonnet] [--lang en|ja]
        Summarize idle sessions now. Summaries use `claude -p` (your Claude Code login).
    agent-calendar service install [--port 23848] [--no-summarize] [--summary-lang en|ja] [--force]
        Linux only: register and start a systemd user service that runs `serve`.
    agent-calendar todo [--from 2026-10-06] [--to 2026-10-07] [--all] [--repo <path>]
        List sessions active in those days (default: today) that are in progress, or not done
        with something left to do in their summary: id, last activity, status, repository, title, what is left.
        --all: any time; --repo <absolute path>: only that repository.
    agent-calendar done <session id> [--undo]
        Mark a session as done (it leaves the todo list and shows as done in the page). The mark lasts until
        the session continues; --undo removes it.
    agent-calendar stats [--from 2026-10-06] [--to 2026-10-12] [--json | --csv]
        Active time, sessions and API-price cost per project and repository (default: today).
    agent-calendar report [--from …] [--to …]
        The same period as Markdown: totals, what is left to do, and the saved period summary if there is one.
    agent-calendar search <words…> [--json]
        Find past sessions on this machine by repository, branch, folder, title, summary, instructions or replies.
    agent-calendar limits
        Show each agent's usage limits (5-hour, weekly, per model) and when they reset, from local records.
    agent-calendar statusline --agent claude|gemini
        Use as the status line command of Claude Code (~/.claude/settings.json) or Antigravity
        (~/.gemini/antigravity-cli/settings.json): it keeps the usage limits it receives for the page and prints them.

  Collect sessions from another machine (over HTTPS with a pinned certificate and a token):
    agent-calendar share [--bind 0.0.0.0:23847]           on the other machine: serve its transcripts
    agent-calendar share pair [--bind …] [--host <ip>]   on the other machine: print the connection string
    agent-calendar service install share [--bind …]       Linux only: run `share` as a systemd user service
    agent-calendar remote add '<connection string>' [--name ai-node]   on this machine: register it
    agent-calendar remote list | remote remove <name>

  Updates (from GitHub Releases, verified with the published sha256):
    agent-calendar update [--check]        install the latest release, or only show whether one is available

The summary language defaults to Japanese when $LANG starts with \"ja\", English otherwise.
";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print!("{USAGE}");
        return Ok(());
    };
    match cmd.as_str() {
        "update" => {
            // 通信は blocking の client なので、非同期の外で行う
            let only_check = args.iter().any(|a| a == "--check");
            tokio::task::spawn_blocking(move || run_update(only_check)).await??;
        }
        "scan" => {
            // 別のマシンの分も取ってから読む（画面の読み直しと同じ）。通信は blocking の client なので非同期の外で
            let n = tokio::task::spawn_blocking(|| -> Result<usize> {
                let mut conn = db::open()?;
                remote::pull_all(&conn)?;
                scan::run(&mut conn)
            })
            .await??;
            println!("[scan] re-read {n} sessions");
        }
        "statusline" => statusline_cmd(&args)?,
        "limits" => limits_cmd()?,
        "stats" => stats_cmd(&args)?,
        "report" => report_cmd(&args)?,
        "search" => search_cmd(&args)?,
        "summarize" => {
            let limit = opt(&args, "--limit")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(20);
            let model = opt(&args, "--model").unwrap_or_else(|| summarize::DEFAULT_MODEL.into());
            let lang = summarize::Lang::from_arg(opt(&args, "--lang").as_deref())?;
            let conn = db::open()?;
            let n = summarize::run(&conn, limit, &model, lang)?;
            println!("[summarize] summarized {n} sessions");
        }
        "todo" => todo_cmd(&args)?,
        "done" => done_cmd(&args)?,
        "serve" => serve::run(&args).await?,
        "share" if args.get(1).map(String::as_str) == Some("pair") => share::pair(&args)?,
        "share" => share::run(&args).await?,
        "remote" => remote_cmd(&args)?,
        "service" if args.get(1).map(String::as_str) == Some("install") => service::install(&args)?,
        "-h" | "--help" | "help" => print!("{USAGE}"),
        other => bail!("unknown command: {other}\n\n{USAGE}"),
    }
    Ok(())
}

/// `agent-calendar statusline --agent claude|gemini`: Claude Code・Antigravity の statusline に登録するコマンド。
/// 渡された JSON から利用枠を残し（画面の「利用枠」に出す）、端末には使い具合を短く出す
fn statusline_cmd(args: &[String]) -> Result<()> {
    use std::io::Read;
    let agent = opt(args, "--agent").unwrap_or_else(|| "claude".into());
    if agent != "claude" && agent != "gemini" {
        bail!("--agent must be claude or gemini");
    }
    let mut input = String::new();
    // statusline の JSON は小さい。念のため 1MB で切る
    std::io::stdin().take(1 << 20).read_to_string(&mut input)?;
    let line = limits::save_statusline(
        &limits::dir(),
        &agent,
        &input,
        chrono::Utc::now().timestamp_millis(),
    )?;
    println!("{line}");
    Ok(())
}

/// `agent-calendar limits`: 各エージェントの利用枠の使い具合と戻る時刻
fn limits_cmd() -> Result<()> {
    let v = limits::all(
        &db::home(),
        &limits::dir(),
        chrono::Utc::now().timestamp_millis(),
    );
    for line in limits_lines(&v) {
        println!("{line}");
    }
    Ok(())
}

/// 利用枠を 1 行ずつ（エージェント・枠・使い具合・戻る時刻・いつの時点か）
fn limits_lines(v: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    for a in v["agents"].as_array().into_iter().flatten() {
        let name = a["agent"].as_str().unwrap_or("");
        let windows = a["windows"].as_array().cloned().unwrap_or_default();
        if windows.is_empty() {
            let why = if a["setup"] == "statusline" {
                "no data (add `agent-calendar statusline` to its status line)"
            } else if a["reset"].as_u64().unwrap_or(0) > 0 {
                "the limits in the last record have reset since"
            } else {
                "no data"
            };
            out.push(format!("{name}\t{why}"));
            continue;
        }
        let at = a["observed_at"]
            .as_i64()
            .map(|t| local_time(t, "%m-%d %H:%M"))
            .unwrap_or_default();
        for w in windows {
            let label = match w["label"].as_str().filter(|l| !l.is_empty()) {
                Some(l) => l.to_string(),
                None => w["kind"].as_str().unwrap_or("").to_string(),
            };
            let used = w["used_percent"]
                .as_f64()
                .map(|p| format!("{p:.0}%"))
                .unwrap_or_else(|| "-".into());
            let reset = w["resets_at"]
                .as_i64()
                .map(|t| local_time(t, "%m-%d %H:%M"))
                .unwrap_or_default();
            out.push(format!(
                "{name}\t{label}\t{used}\tresets {reset}\t(as of {at})"
            ));
        }
    }
    out
}

/// `agent-calendar stats [--from 日付] [--to 日付] [--json | --csv]`: 案件・リポジトリごとの作業時間と費用
fn stats_cmd(args: &[String]) -> Result<()> {
    let (from, to) = day_range(args)?;
    let conn = db::open()?;
    let v = stats::stats(&conn, from, to, None, &[])?;
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let csv = args.iter().any(|a| a == "--csv");
    print!("{}", stats::table(&v, csv));
    Ok(())
}

/// `agent-calendar report [--from 日付] [--to 日付]`: その期間の集計と残タスク、保存した期間の要約を Markdown で
fn report_cmd(args: &[String]) -> Result<()> {
    let (from, to) = day_range(args)?;
    let conn = db::open()?;
    let v = stats::stats(&conn, from, to, None, &[])?;
    let (todo, pending) = finished::todo(
        &conn,
        &finished::Query {
            range: Some((from, to)),
            ..Default::default()
        },
    )?;
    let saved = period::load(&conn, from, to, None)?.map(|s| s.text);
    print!(
        "{}",
        stats::report(&v, &todo, pending.len(), saved.as_deref(), (from, to))
    );
    Ok(())
}

/// `agent-calendar search <語…> [--json]`: このマシンの過去のセッションを探す（新しい順に 50 件）
fn search_cmd(args: &[String]) -> Result<()> {
    let json = args.iter().any(|a| a == "--json");
    let q = args
        .iter()
        .skip(1)
        .filter(|a| *a != "--json")
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if q.trim().is_empty() {
        bail!("usage: agent-calendar search <words…> [--json]");
    }
    let conn = db::open()?;
    let found = search::sessions(&conn, &q, &search::Filter::default(), 50)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&found)?);
        return Ok(());
    }
    for s in &found {
        let repo = s["repo"].as_str().unwrap_or("");
        println!(
            "{}	{}	{}	{}	{}",
            s["id"].as_str().unwrap_or(""),
            local_time(s["last_ts"].as_i64().unwrap_or(0), "%Y-%m-%d %H:%M"),
            repo.rsplit('/').next().unwrap_or(repo),
            s["branch"].as_str().unwrap_or(""),
            s["title"].as_str().unwrap_or(""),
        );
    }
    Ok(())
}

fn remote_cmd(args: &[String]) -> Result<()> {
    let conn = db::open()?;
    match (args.get(1).map(String::as_str), args.get(2)) {
        // reqwest の blocking は tokio の中で直接呼べないので、別のスレッドで
        (Some("add"), Some(s)) => {
            let (s, name) = (s.clone(), opt(args, "--name"));
            std::thread::spawn(move || remote::add(&db::open()?, &s, name.as_deref()).map(|_| ()))
                .join()
                .map_err(|_| anyhow::anyhow!("remote add crashed"))??;
        }
        (Some("list"), _) => {
            for r in remote::list(&conn)? {
                println!("{}\t{}", r.name, r.url);
            }
        }
        (Some("remove"), Some(name)) => {
            if !remote::remove(&conn, name)? {
                bail!("no remote named {name}");
            }
            println!("removed {name} (sessions already collected stay on the calendar)");
        }
        _ => bail!(
            "usage: agent-calendar remote add '<connection string>' [--name N] | remote list | remote remove <name>"
        ),
    }
    Ok(())
}

/// `--from 日付 --to 日付` の範囲（unix 秒。to はその日を含む）。無ければ今日だけ
fn day_range(args: &[String]) -> Result<(i64, i64)> {
    let day = |flag: &str| -> Result<Option<chrono::NaiveDate>> {
        opt(args, flag)
            .map(|v| {
                chrono::NaiveDate::parse_from_str(&v, "%Y-%m-%d")
                    .map_err(|_| anyhow::anyhow!("{flag} takes a date like 2026-10-06, got {v}"))
            })
            .transpose()
    };
    let from = day("--from")?.unwrap_or_else(|| chrono::Local::now().date_naive());
    let to = day("--to")?.unwrap_or(from);
    if to < from {
        bail!("--to is before --from");
    }
    let secs = |d: chrono::NaiveDate| -> Result<i64> {
        d.and_hms_opt(0, 0, 0)
            .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
            .map(|t| t.timestamp())
            .ok_or_else(|| anyhow::anyhow!("no local midnight on {d}"))
    };
    Ok((secs(from)?, secs(to + chrono::Days::new(1))?))
}

fn local_time(ms: i64, fmt: &str) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&chrono::Local).format(fmt).to_string())
        .unwrap_or_default()
}

/// `agent-calendar todo [--from 日付] [--to 日付] [--all] [--repo パス]`: その日々に動いたセッションの残タスク。
/// to はその日を含む。--all なら期間を問わない。--repo でリポジトリ（絶対パス）を絞る
fn todo_cmd(args: &[String]) -> Result<()> {
    let conn = db::open()?;
    let repo = opt(args, "--repo");
    let range = if args.iter().any(|a| a == "--all") {
        None
    } else {
        Some(day_range(args)?)
    };
    let (list, pending) = finished::todo(
        &conn,
        &finished::Query {
            range,
            repo: repo.as_deref(),
            ..Default::default()
        },
    )?;
    let unsummarized = pending.len();
    for x in &list {
        let when = local_time(x.last_ts, "%m-%d %H:%M");
        let repo = x.repo.rsplit('/').next().unwrap_or(&x.repo);
        println!(
            "{}\t{when}\t{}\t{repo}\t{}\t{}",
            x.id, x.status, x.title, x.next
        );
    }
    println!("[todo] {} sessions with something left", list.len());
    if unsummarized > 0 {
        println!(
            "[todo] {unsummarized} sessions are not summarized yet (agent-calendar summarize)"
        );
    }
    Ok(())
}

/// `agent-calendar done <id> [--undo]`（`--undo` は前でも後でもよい）
fn done_cmd(args: &[String]) -> Result<()> {
    let Some(id) = args.iter().skip(1).find(|a| !a.starts_with("--")) else {
        bail!("usage: agent-calendar done <session id> [--undo]");
    };
    let undo = args.iter().any(|a| a == "--undo");
    let conn = db::open()?;
    if !finished::set(&conn, id, !undo, chrono::Utc::now().timestamp_millis())? {
        bail!("no session {id}");
    }
    println!(
        "{id}: {}",
        if undo { "mark removed" } else { "marked done" }
    );
    Ok(())
}

pub fn opt(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .filter(|v| !v.starts_with("--"))
        .cloned()
}

/// `agent-calendar update [--check]`: 最新のリリースを確かめ、新しければ（--check でなければ）入れ替える
fn run_update(only_check: bool) -> Result<()> {
    let latest = update::check()?;
    println!(
        "installed {} / latest {} ({})",
        update::CURRENT,
        latest.version,
        latest.page
    );
    if !update::is_newer(&latest.version, update::CURRENT) {
        println!("already up to date");
        return Ok(());
    }
    if only_check {
        return Ok(());
    }
    let work = std::env::temp_dir().join(format!("agent-calendar-update-{}", std::process::id()));
    let installed = update::install(&latest, &work);
    let _ = std::fs::remove_dir_all(&work);
    let exe = installed?;
    println!(
        "updated {} to {}. restart it (Linux: systemctl --user restart agent-calendar)",
        exe.display(),
        latest.version
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reads_day_ranges() {
        let (from, to) = day_range(&args(&[
            "stats",
            "--from",
            "2026-10-06",
            "--to",
            "2026-10-07",
        ]))
        .unwrap();
        assert_eq!(to - from, 2 * 86400); // to の日を含む（夏時間の無いこのマシンの時刻で）
        assert_eq!(
            local_time(from * 1000, "%Y-%m-%d %H:%M"),
            "2026-10-06 00:00"
        );
        assert!(day_range(&args(&["stats", "--from", "10/06"])).is_err());
        assert!(
            day_range(&args(&[
                "stats",
                "--from",
                "2026-10-07",
                "--to",
                "2026-10-06"
            ]))
            .is_err()
        );
        let (from, to) = day_range(&args(&["stats"])).unwrap(); // 今日
        assert_eq!(to - from, 86400);
    }

    #[test]
    fn prints_limits() {
        let v = serde_json::json!({ "agents": [
            { "agent": "claude", "windows": [], "setup": "statusline", "reset": 0 },
            { "agent": "codex", "windows": [], "setup": null, "reset": 1 },
            { "agent": "muse", "windows": [], "setup": null, "reset": 0 },
            { "agent": "gemini", "observed_at": 0, "setup": null, "reset": 0, "windows": [
                { "kind": "model", "label": "gemini-4-argon-high", "used_percent": 100.0, "resets_at": 0 },
                { "kind": "five_hour", "label": "", "used_percent": null, "resets_at": null },
            ] },
        ] });
        let lines = limits_lines(&v);
        assert!(lines[0].contains("statusline"));
        assert!(lines[1].contains("have reset since"));
        assert_eq!(lines[2], "muse\tno data");
        assert!(lines[3].starts_with("gemini\tgemini-4-argon-high\t100%\tresets "));
        assert!(lines[4].starts_with("gemini\tfive_hour\t-\tresets \t"));
    }
}
