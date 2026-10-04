//! agent-calendar — a local calendar for AI coding agent sessions (Claude Code and Codex).
//!
//! Reads the session transcripts that Claude Code and Codex write to your home directory,
//! keeps a SQLite index, and serves a calendar / stats page on 127.0.0.1.

mod codex;
mod db;
mod diff;
mod github;
mod pins;
mod pricing;
mod remote;
mod repo;
mod resume;
mod scan;
mod schedule;
mod send;
mod serve;
mod service;
mod settings;
mod share;
mod stats;
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
    agent-calendar service install [--port 23848] [--no-summarize] [--summary-lang en|ja]
        Linux only: register and start a systemd user service that runs `serve`.

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
            let mut conn = db::open()?;
            let n = scan::run(&mut conn)?;
            println!("[scan] re-read {n} sessions");
        }
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
    let exe = update::install(&latest, &work)?;
    println!(
        "updated {} to {}. restart it (Linux: systemctl --user restart agent-calendar)",
        exe.display(),
        latest.version
    );
    Ok(())
}
