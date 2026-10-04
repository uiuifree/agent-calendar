use crate::{db, opt};
use anyhow::{Context, Result, bail};
use std::process::Command;

/// systemd のユーザーサービスとして登録して起動する（Linux だけ）。
/// `service install` は serve（画面）、`service install share` は別のマシン用の share
pub fn install(args: &[String]) -> Result<()> {
    let share = args.get(2).map(String::as_str) == Some("share");
    let unit = if share {
        "agent-calendar-share.service"
    } else {
        "agent-calendar.service"
    };
    if !cfg!(target_os = "linux") {
        bail!(
            "`service install` is Linux only; run `agent-calendar serve` from your login items instead"
        );
    }
    let exe = std::env::current_exe().context("cannot find this executable")?;
    let path = std::env::var("PATH").unwrap_or_default();
    let cmd = if share {
        share_args(args)
    } else {
        serve_args(args)
    };
    let text = unit_text(&exe.to_string_lossy(), &cmd, &path);
    let dir = db::home().join(".config/systemd/user");
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let file = dir.join(unit);
    std::fs::write(&file, text).with_context(|| format!("cannot write {}", file.display()))?;
    for step in [
        &["daemon-reload"][..],
        &["enable", "--now", unit][..],
        &["restart", unit][..],
    ] {
        let ok = Command::new("systemctl")
            .arg("--user")
            .args(step)
            .status()
            .context("cannot run systemctl")?;
        if !ok.success() {
            bail!("systemctl --user {} failed", step.join(" "));
        }
    }
    if share {
        println!(
            "installed {}\nnext: run `agent-calendar share pair` and paste the line on the calendar machine",
            file.display()
        );
    } else {
        let port = opt(args, "--port").unwrap_or_else(|| "8082".into());
        println!(
            "installed {}\nopen http://127.0.0.1:{port}/",
            file.display()
        );
    }
    Ok(())
}

fn share_args(args: &[String]) -> Vec<String> {
    let mut out = vec!["share".to_string()];
    if let Some(v) = opt(args, "--bind") {
        out.extend(["--bind".to_string(), v]);
    }
    out
}

/// `service install` に渡されたもののうち、serve にそのまま渡すもの
fn serve_args(args: &[String]) -> Vec<String> {
    let mut out = vec!["serve".to_string()];
    for flag in ["--port", "--summary-model", "--summary-lang"] {
        if let Some(v) = opt(args, flag) {
            out.extend([flag.to_string(), v]);
        }
    }
    if args.iter().any(|a| a == "--no-summarize") {
        out.push("--no-summarize".into());
    }
    out
}

/// 要約で claude を呼ぶので、入れたときのシェルの PATH を渡しておく。
/// WSL2 の PATH には `/mnt/c/Program Files/...` のような空白が入るので、引用符で囲む
/// （囲まないと systemd は空白で区切り、PATH が途中で切れる）。`%` は systemd の置き換え記号なので `%%` にする
fn unit_text(exe: &str, args: &[String], path: &str) -> String {
    let path = path
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    let quote = |s: &str| {
        if s.contains(' ') {
            format!("\"{s}\"")
        } else {
            s.to_string()
        }
    };
    let cmd = std::iter::once(quote(exe))
        .chain(args.iter().map(|a| quote(a)))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Unit]\nDescription=agent-calendar (calendar for AI coding agent sessions)\nAfter=network.target\n\n\
         [Service]\nType=simple\nExecStart={cmd}\nRestart=always\nRestartSec=3\nEnvironment=\"PATH={path}\"\nEnvironment=HOME=%h\n\n\
         [Install]\nWantedBy=default.target\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn passes_serve_flags_through() {
        assert_eq!(serve_args(&args(&["service", "install"])), ["serve"]);
        assert_eq!(
            serve_args(&args(&[
                "service",
                "install",
                "--port",
                "9000",
                "--summary-lang",
                "ja",
                "--no-summarize"
            ])),
            [
                "serve",
                "--port",
                "9000",
                "--summary-lang",
                "ja",
                "--no-summarize"
            ]
        );
    }

    #[test]
    fn share_unit_args() {
        assert_eq!(
            share_args(&args(&["service", "install", "share"])),
            ["share"]
        );
        assert_eq!(
            share_args(&args(&[
                "service",
                "install",
                "share",
                "--bind",
                "10.0.0.2:9443"
            ])),
            ["share", "--bind", "10.0.0.2:9443"]
        );
    }

    #[test]
    fn unit_runs_serve_with_path() {
        let t = unit_text(
            "/home/u/.local/bin/agent-calendar",
            &args(&["serve", "--port", "9000"]),
            "/usr/bin:/bin",
        );
        assert!(t.contains("ExecStart=/home/u/.local/bin/agent-calendar serve --port 9000\n"));
        assert!(t.contains("Environment=\"PATH=/usr/bin:/bin\"\n"));
        let wsl = unit_text("/x", &[], "/usr/bin:/mnt/c/Program Files/a%b");
        assert!(
            wsl.contains("Environment=\"PATH=/usr/bin:/mnt/c/Program Files/a%%b\"\n"),
            "{wsl}"
        );
        assert!(t.contains("WantedBy=default.target"));
        assert!(
            unit_text("/a b/agent-calendar", &[], "")
                .contains("ExecStart=\"/a b/agent-calendar\"\n")
        );
    }
}
