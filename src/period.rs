//! 期間（日・週・月）の要約。集計の画面のボタンで作り、期間とホストの絞り込みごとに保存する。
//! 材料は各セッションの要約（題・やったこと・状態・残っていること）で、記録そのものは読まない
use crate::{finished, pins, summarize};
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

#[derive(Debug, PartialEq, Serialize)]
pub struct Saved {
    pub text: String,
    pub model: String,
    pub created_at: i64,
    pub cost_usd: Option<f64>,
}

fn instruction(lang: summarize::Lang) -> &'static str {
    match lang {
        summarize::Lang::En => {
            r#"Below is a list of coding agent sessions (Claude Code, Codex) in one period, with each session's own summary.
Write a summary of the whole period in Markdown, no preface:
## Overview
3 to 5 short bullets, one line each, on what the period was about and where things stand. This part is shown first, so keep it brief.
## By repository
A ### heading with the repository name for each, then bullets of what was done or decided.
## Left to do
Bullets of what is still open, with the repository. Skip sessions whose status is done. If nothing is open, say so.
Write plainly so the behavior and results are clear. File names, commands and identifiers may stay as they are."#
        }
        summarize::Lang::Ja => {
            r#"次は、ある期間にコーディングエージェント（Claude Code・Codex）で行ったセッションの一覧と、各セッションの要約です。
期間全体の要約をマークダウンで書く。前置きは書かない。
## 概要
期間に何をして、いまどこまで進んでいるかを、1 行ずつの短い箇条書き 3〜5 個で。ここだけ最初に見せるので端的に。
## リポジトリごと
リポジトリごとに「### リポジトリ名」の見出しを置き、やったこと・決めたことを箇条書きで。
## 残タスク
まだ終わっていないことを、リポジトリ名を添えて箇条書きで。状態が完了のセッションの分は入れない。無ければ無いと書く。
挙動と結果がそのまま伝わる平易な言葉で書く。ファイル名・コマンド・識別子はそのまま書いてよい。"#
        }
    }
}

/// 保存の鍵にするホストの絞り込み。すべてなら空文字、選んでいればその並べ替えた JSON
fn key(machines: Option<&[String]>) -> String {
    machines.map_or_else(String::new, |m| {
        let mut m = m.to_vec();
        m.sort();
        serde_json::Value::from(m).to_string()
    })
}

/// 保存した要約（from・to は unix 秒）
pub fn load(
    conn: &Connection,
    from: i64,
    to: i64,
    machines: Option<&[String]>,
) -> Result<Option<Saved>> {
    Ok(conn
        .query_row(
            "SELECT body, model, created_at, cost_usd FROM period_summaries
             WHERE from_ts = ?1 AND to_ts = ?2 AND machines = ?3",
            params![from, to, key(machines)],
            |r| {
                Ok(Saved {
                    text: r.get(0)?,
                    model: r.get(1)?,
                    created_at: r.get(2)?,
                    cost_usd: r.get(3)?,
                })
            },
        )
        .optional()?)
}

/// claude に渡す本文。その期間に動いたセッションを始まった順に。1 件も無ければ None
pub fn input(
    conn: &Connection,
    from: i64,
    to: i64,
    machines: Option<&[String]>,
) -> Result<Option<String>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT s.repo, s.title, m.body,
                (SELECT text FROM prompts p WHERE p.session_id = s.id ORDER BY seq LIMIT 1),
                s.first_ts, s.last_ts, s.machine, {}
         FROM sessions s LEFT JOIN summaries m ON m.session_id = s.id
         WHERE EXISTS (SELECT 1 FROM activity a WHERE a.session_id = s.id AND a.bucket >= ?1 AND a.bucket < ?2)
         ORDER BY s.first_ts",
        finished::SQL
    ))?;
    let mut rows = stmt.query([from, to])?;
    let at = |ms: i64| {
        chrono::DateTime::from_timestamp_millis(ms)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default()
    };
    let mut out = vec![format!(
        "period: {} - {}",
        at(from * 1000),
        at(to * 1000 - 1)
    )];
    while let Some(r) = rows.next()? {
        let machine: String = r.get(6)?;
        if machines.is_some_and(|m| !m.contains(&machine)) {
            continue;
        }
        let repo: String = r.get(0)?;
        let summary = r
            .get::<_, Option<String>>(2)?
            .and_then(|b| summarize::read(&b));
        let status = finished::status(summary.as_ref(), r.get(7)?);
        let mut s = format!(
            "- [{}] {} ({} - {}) status: {}",
            repo.rsplit('/').next().unwrap_or(&repo),
            pins::title(summary.as_ref(), r.get(1)?, r.get(3)?),
            at(r.get(4)?),
            at(r.get(5)?),
            if status.is_empty() {
                "not summarized"
            } else {
                &status
            },
        );
        if let Some(x) = &summary {
            for b in &x.bullets {
                s.push_str(&format!("\n  - {b}"));
            }
            if !x.next.is_empty() && status != "done" {
                s.push_str(&format!("\n  left: {}", x.next));
            }
        }
        out.push(s);
    }
    Ok((out.len() > 1).then(|| out.join("\n")))
}

/// 要約を作って保存する（同じ期間・同じ絞り込みのものは置き換える）
pub fn make(
    conn: &Connection,
    claude: &str,
    (from, to): (i64, i64),
    machines: Option<&[String]>,
    model: &str,
    lang: summarize::Lang,
) -> Result<Saved> {
    let Some(_busy) = summarize::claim(&format!("period:{from}:{to}:{}", key(machines))) else {
        bail!("this period is already being summarized");
    };
    let Some(text) = input(conn, from, to, machines)? else {
        bail!("no sessions in this period");
    };
    let (text, cost_usd) = summarize::run_claude(claude, &text, model, instruction(lang))?;
    let saved = Saved {
        text: text.trim().to_string(),
        model: model.to_string(),
        created_at: chrono::Utc::now().timestamp_millis(),
        cost_usd,
    };
    conn.execute(
        "INSERT OR REPLACE INTO period_summaries (from_ts, to_ts, machines, body, model, created_at, cost_usd)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![from, to, key(machines), saved.text, saved.model, saved.created_at, saved.cost_usd],
    )?;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(conn: &Connection, id: &str, machine: &str, bucket: i64, body: Option<&str>) {
        conn.execute(
            "INSERT INTO sessions VALUES (?1, ?1, 0, 0, '/r/app', '/r/app', 'main', 'AI の題', ?2, ?2, 'u1', 1, 'claude', ?3)",
            params![id, bucket * 1000, machine],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO activity VALUES (?1, ?2, 1)",
            params![id, bucket],
        )
        .unwrap();
        if let Some(b) = body {
            conn.execute(
                "INSERT INTO summaries (session_id, last_uuid, model, body, created_at) VALUES (?1, 'u1', 'm', ?2, 0)",
                params![id, b],
            )
            .unwrap();
        }
    }

    /// claude の代わりに、受け取った本文の行数を返事にするスクリプト
    fn fake_claude(dir: &std::path::Path, code: i32) -> String {
        let p = dir.join(format!("claude-{code}"));
        crate::db::write_script(
            &p,
            &format!(
                r#"#!/bin/sh
n=$(wc -l)
printf '{{"result":"  ## 概要\\n%s 行\\n","total_cost_usd":0.02}}' "$n"
exit {code}
"#
            ),
        );
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn input_lists_sessions_with_their_summaries() {
        let conn = crate::db::open_at(&crate::db::temp_dir("period-input").join("t.db")).unwrap();
        assert_eq!(input(&conn, 0, 3000, None).unwrap(), None);
        session(
            &conn,
            "a",
            "",
            600,
            Some(r#"{"title":"題A","bullets":["直した"],"status":"wip","next":"push"}"#),
        );
        session(
            &conn,
            "b",
            "ai-node",
            1200,
            Some(r#"{"title":"題B","bullets":[],"status":"完了","next":"確認"}"#),
        );
        session(&conn, "c", "", 1800, None);
        session(&conn, "late", "", 9000, None);
        let text = input(&conn, 0, 3000, None).unwrap().unwrap();
        let lines: Vec<_> = text.lines().skip(1).collect();
        assert!(lines[0].starts_with("- [app] 題A ("), "{text}");
        assert!(lines[0].ends_with("status: wip"));
        assert_eq!(lines[1..3], ["  - 直した", "  left: push"]);
        // 完了のものには残りを付けない
        assert!(lines[3].starts_with("- [app] 題B (") && lines[3].ends_with("status: done"));
        assert!(
            lines[4].starts_with("- [app] AI の題 (")
                && lines[4].ends_with("status: not summarized")
        );
        assert_eq!(lines.len(), 5);
        // 完了の印が付いていれば完了として渡す
        finished::set(&conn, "a", true, 1_000_000).unwrap();
        let text = input(&conn, 0, 3000, Some(&[String::new()]))
            .unwrap()
            .unwrap();
        assert!(
            text.contains("題A") && text.contains("status: done") && !text.contains("left: push")
        );
        assert!(!text.contains("題B"), "{text}");
    }

    #[test]
    fn makes_and_keeps_one_per_period_and_hosts() {
        let dir = crate::db::temp_dir("period-make");
        let conn = crate::db::open_at(&dir.join("t.db")).unwrap();
        let (ok, ng) = (fake_claude(&dir, 0), fake_claude(&dir, 1));
        let ja = summarize::Lang::Ja;
        assert!(make(&conn, &ok, (0, 3000), None, "sonnet", ja).is_err()); // セッションが無い
        session(&conn, "a", "", 600, None);
        let s = make(&conn, &ok, (0, 3000), None, "sonnet", ja).unwrap();
        assert_eq!(
            (s.text.as_str(), s.model.as_str(), s.cost_usd),
            ("## 概要\n1 行", "sonnet", Some(0.02))
        );
        assert_eq!(load(&conn, 0, 3000, None).unwrap(), Some(s));
        // ホストの絞り込みごとに別に持つ。並び順は問わない
        let hosts = ["b".to_string(), String::new()];
        assert_eq!(load(&conn, 0, 3000, Some(&hosts)).unwrap(), None);
        make(
            &conn,
            &ok,
            (0, 3000),
            Some(&hosts),
            "haiku",
            summarize::Lang::En,
        )
        .unwrap();
        let swapped = [String::new(), "b".to_string()];
        assert_eq!(
            load(&conn, 0, 3000, Some(&swapped)).unwrap().unwrap().model,
            "haiku"
        );
        // 失敗したら前の要約を残す
        assert!(make(&conn, &ng, (0, 3000), None, "opus", ja).is_err());
        assert_eq!(load(&conn, 0, 3000, None).unwrap().unwrap().model, "sonnet");
        // 作っている最中は同じ期間をもう一度作らない
        let _busy = summarize::claim("period:0:3000:").unwrap();
        assert!(make(&conn, &ok, (0, 3000), None, "sonnet", ja).is_err());
    }

    #[test]
    fn instructions_ask_for_the_same_sections() {
        assert!(instruction(summarize::Lang::En).contains("## Left to do"));
        assert!(instruction(summarize::Lang::Ja).contains("## 残タスク"));
    }
}
