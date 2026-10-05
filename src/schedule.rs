//! 予定: 決めた時刻に、決めたディレクトリで、決めた指示をエージェントに実行させる。
//! 実行は `send::start`（画面からの続きの指示と同じ仕組み）。PC が止まっていて逃した回は実行しない
use crate::{db, send};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

/// 予定の時刻からこれ以上遅れていたら、逃したと見て実行しない（PC が止まっていた・寝ていた）
const LATE_MS: i64 = 5 * 60 * 1000;

/// いつ動かすか。days は月曜を 1 ビット目とする曜日の印（月〜日 = 0b111_1111）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Repeat {
    /// 1 回だけ（unix ms）
    Once { at: i64 },
    /// 選んだ曜日の決まった時刻（0:00 からの分）
    Weekly { days: u8, minute: u32 },
    /// 選んだ曜日の from_hour〜to_hour のあいだ、every_min ごと（from_hour ちょうどから数える）
    Interval {
        days: u8,
        every_min: u32,
        from_hour: u32,
        to_hour: u32,
    },
}

fn day_on(days: u8, date: NaiveDate) -> bool {
    days & (1 << date.weekday().num_days_from_monday()) != 0
}

fn at(date: NaiveDate, minute: u32) -> Option<DateTime<Local>> {
    let naive = date.and_hms_opt(minute / 60 % 24, minute % 60, 0)?;
    // 24:00 は翌日の 0:00 として扱う
    let naive = if minute >= 24 * 60 {
        naive + ChronoDuration::days(1)
    } else {
        naive
    };
    Local.from_local_datetime(&naive).earliest()
}

/// after より後で、次に動く時刻。もう動かないなら None
pub fn next_after(r: &Repeat, after: DateTime<Local>) -> Option<DateTime<Local>> {
    match *r {
        Repeat::Once { at: ms } => Local
            .timestamp_millis_opt(ms)
            .single()
            .filter(|t| *t > after),
        Repeat::Weekly { days, minute } => (0..8)
            .map(|d| after.date_naive() + ChronoDuration::days(d))
            .filter(|date| day_on(days, *date))
            .filter_map(|date| at(date, minute))
            .find(|t| *t > after),
        Repeat::Interval {
            days,
            every_min,
            from_hour,
            to_hour,
        } => (0..8)
            .map(|d| after.date_naive() + ChronoDuration::days(d))
            .filter(|date| day_on(days, *date))
            .flat_map(|date| {
                (from_hour * 60..to_hour * 60)
                    .step_by(every_min.max(1) as usize)
                    .filter_map(move |m| at(date, m))
            })
            .find(|t| *t > after),
    }
}

/// from 以上 to 未満で動く時刻。時間ごとの予定は 1 日 1 つの帯（その日の最初の回〜時間帯の終わり）にまとめる
fn occurrences(
    r: &Repeat,
    from: DateTime<Local>,
    to: DateTime<Local>,
) -> Vec<(DateTime<Local>, Option<DateTime<Local>>)> {
    let mut out = Vec::new();
    let mut after = from - ChronoDuration::milliseconds(1);
    while let Some(t) = next_after(r, after).filter(|t| *t < to) {
        let Repeat::Interval { to_hour, .. } = *r else {
            out.push((t, None));
            after = t;
            continue;
        };
        let Some(end) = at(t.date_naive(), to_hour * 60) else {
            break;
        };
        out.push((t, Some(end)));
        // その日の残りの回は帯に含めたので、次は時間帯が終わってから
        after = end - ChronoDuration::milliseconds(1);
    }
    out
}

/// カレンダーに出す、これから動く予定（止めている予定は出さない）
pub fn upcoming(
    conn: &Connection,
    from: DateTime<Local>,
    to: DateTime<Local>,
) -> Result<Vec<Value>> {
    let mut stmt = conn
        .prepare("SELECT id, name, agent, repeat FROM schedules WHERE enabled = 1 ORDER BY id")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (id, name, agent, repeat) in rows {
        let repeat: Repeat = serde_json::from_str(&repeat).context("broken schedule")?;
        let every_min = match repeat {
            Repeat::Interval { every_min, .. } => Some(every_min),
            _ => None,
        };
        for (start, end) in occurrences(&repeat, from, to) {
            out.push(json!({
                "id": id, "name": name, "agent": agent, "every_min": every_min,
                "start": start.timestamp_millis(), "end": end.map(|e| e.timestamp_millis()),
            }));
        }
    }
    Ok(out)
}

/// カレンダーに予定として残す、まだ正常に終わっていない実行（実行中・失敗・実行されず・飛ばした）。
/// 正常に終わった回はセッションとして出るので、予定からは消す。
/// 失敗した回も、次の予定時刻より前にやり直して正常に終わっていれば消す（1 回だけの予定は、あとで 1 度でも）。
/// 前の回がまだ動いていて飛ばした回は、飛ばした時点で動いていた回が正常に終われば消す
pub fn unfinished_runs(conn: &Connection, from_ms: i64, to_ms: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.name, s.agent, r.started_at, r.status, s.repeat, r.id FROM schedule_runs r JOIN schedules s ON s.id = r.schedule_id
         WHERE r.status != 'ok' AND r.started_at >= ?1 AND r.started_at < ?2 ORDER BY r.started_at, r.id",
    )?;
    let rows = stmt
        .query_map([from_ms, to_ms], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut redone = conn.prepare(
        "SELECT 1 FROM schedule_runs WHERE schedule_id = ?1 AND status = 'ok' AND started_at > ?2 AND started_at < ?3 LIMIT 1",
    )?;
    // 飛ばした時点で動いていた回（その前に始まり、その後に終わった回）が正常に終わったか
    let mut overlapped = conn.prepare(
        "SELECT 1 FROM schedule_runs WHERE schedule_id = ?1 AND status = 'ok' AND started_at <= ?2 AND finished_at >= ?2 AND id != ?3 LIMIT 1",
    )?;
    let mut out = Vec::new();
    for (id, name, agent, start, status, repeat, run_id) in rows {
        let repeat: Repeat = serde_json::from_str(&repeat).context("broken schedule")?;
        // 1 回だけの予定は、時刻を変えてやり直すと次の時刻がやり直しの回そのものになるので、区切らない
        let next = match repeat {
            Repeat::Once { .. } => i64::MAX,
            _ => Local
                .timestamp_millis_opt(start)
                .single()
                .and_then(|t| next_after(&repeat, t))
                .map_or(i64::MAX, |t| t.timestamp_millis()),
        };
        if redone.exists(params![id, start, next])?
            || (status == "skipped" && overlapped.exists(params![id, start, run_id])?)
        {
            continue;
        }
        out.push(json!({
            "id": id, "name": name, "agent": agent, "start": start,
            "end": Value::Null, "every_min": Value::Null, "status": status,
        }));
    }
    Ok(out)
}

/// 画面から受け取る予定
#[derive(Debug, Clone, Deserialize)]
pub struct Input {
    pub name: String,
    pub dir: String,
    /// claude / codex
    pub agent: String,
    pub mode: send::Mode,
    /// 予定専用の git worktree で動かす
    pub worktree: bool,
    /// 前回のセッションに続けて指示する（false なら毎回新しく始める）
    pub continue_session: bool,
    pub prompt: String,
    pub repeat: Repeat,
    pub enabled: bool,
}

/// 受け取った予定を確かめる（人が入力したものなので、ここで止める）
pub fn validate(i: &Input, now: DateTime<Local>) -> Result<()> {
    if i.name.trim().is_empty() || i.name.chars().count() > 100 {
        bail!("the name must be 1-100 characters");
    }
    if !Path::new(&i.dir).is_dir() {
        bail!("the directory does not exist: {}", i.dir);
    }
    if i.agent != "claude" && i.agent != "codex" {
        bail!("the agent must be claude or codex");
    }
    if i.prompt.trim().is_empty() || i.prompt.chars().count() > send::MAX_PROMPT_CHARS {
        bail!(
            "the instruction must be 1-{} characters",
            send::MAX_PROMPT_CHARS
        );
    }
    if i.worktree && git_toplevel(&i.dir).is_none() {
        bail!("a worktree needs a git repository: {}", i.dir);
    }
    match i.repeat {
        Repeat::Once { at } if at <= now.timestamp_millis() => bail!("the time is already past"),
        Repeat::Weekly { days, minute } if days & 0x7f == 0 || minute >= 24 * 60 => {
            bail!("pick at least one day and a time")
        }
        Repeat::Interval {
            days,
            every_min,
            from_hour,
            to_hour,
        } if days & 0x7f == 0 || every_min < 15 || from_hour >= to_hour || to_hour > 24 => {
            bail!(
                "pick at least one day, an interval of 15 minutes or more, and a start hour before the end hour"
            )
        }
        _ => Ok(()),
    }
}

pub fn save(conn: &Connection, id: Option<i64>, i: &Input, now: DateTime<Local>) -> Result<i64> {
    validate(i, now)?;
    let next = i
        .enabled
        .then(|| next_after(&i.repeat, now))
        .flatten()
        .map(|t| t.timestamp_millis());
    let repeat = serde_json::to_string(&i.repeat)?;
    let mode = serde_json::to_value(i.mode)?
        .as_str()
        .unwrap_or("read")
        .to_string();
    match id {
        None => {
            conn.execute(
                "INSERT INTO schedules (name, dir, agent, mode, worktree, continue_session, prompt, repeat, enabled, next_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![i.name.trim(), i.dir, i.agent, mode, i.worktree, i.continue_session, i.prompt.trim(), repeat, i.enabled, next, now.timestamp_millis()],
            )?;
            Ok(conn.last_insert_rowid())
        }
        Some(id) => {
            let n = conn.execute(
                // エージェント・ディレクトリ・作業コピーを変えたら、前回のセッションには続けられない
                // （Claude のセッションを Codex では開けず、別の場所の会話も続けられない）。右辺の列は変更前の値
                "UPDATE schedules SET name = ?2, dir = ?3, agent = ?4, mode = ?5, worktree = ?6, continue_session = ?7,
                 prompt = ?8, repeat = ?9, enabled = ?10, next_at = ?11,
                 last_session_id = CASE WHEN agent != ?4 OR dir != ?3 OR worktree != ?6 THEN NULL ELSE last_session_id END
                 WHERE id = ?1",
                params![id, i.name.trim(), i.dir, i.agent, mode, i.worktree, i.continue_session, i.prompt.trim(), repeat, i.enabled, next],
            )?;
            if n == 0 {
                bail!("no such schedule");
            }
            Ok(id)
        }
    }
}

/// そのフォルダを作業ディレクトリにしている予定（あれば 1 つ）。予定が使うフォルダを消させないために見る
pub fn using_dir(conn: &Connection, dir: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM schedules WHERE dir = ?1 LIMIT 1",
            [dir],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    conn.execute("DELETE FROM schedule_runs WHERE schedule_id = ?1", [id])?;
    Ok(conn.execute("DELETE FROM schedules WHERE id = ?1", [id])? > 0)
}

/// 一覧（画面用）。各予定に最近の実行を 5 件付ける
pub fn list(conn: &Connection) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, dir, agent, mode, worktree, continue_session, prompt, repeat, enabled, next_at, last_session_id
         FROM schedules ORDER BY enabled DESC, next_at IS NULL, next_at, id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "dir": r.get::<_, String>(2)?,
                "agent": r.get::<_, String>(3)?, "mode": r.get::<_, String>(4)?, "worktree": r.get::<_, bool>(5)?,
                "continue_session": r.get::<_, bool>(6)?, "prompt": r.get::<_, String>(7)?,
                "repeat": serde_json::from_str::<Value>(&r.get::<_, String>(8)?).unwrap_or(Value::Null),
                "enabled": r.get::<_, bool>(9)?, "next_at": r.get::<_, Option<i64>>(10)?,
                "last_session_id": r.get::<_, Option<String>>(11)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for mut row in rows {
        row["runs"] = runs(conn, row["id"].as_i64().unwrap_or(0), 5)?.into();
        out.push(row);
    }
    Ok(out)
}

pub fn runs(conn: &Connection, id: i64, limit: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, started_at, finished_at, status, session_id, summary, branch, changes
         FROM schedule_runs WHERE schedule_id = ?1 ORDER BY started_at DESC, id DESC LIMIT ?2",
    )?;
    let v = stmt
        .query_map(params![id, limit], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "started_at": r.get::<_, i64>(1)?, "finished_at": r.get::<_, Option<i64>>(2)?,
                "status": r.get::<_, String>(3)?, "session_id": r.get::<_, Option<String>>(4)?,
                "summary": r.get::<_, String>(5)?, "branch": r.get::<_, String>(6)?, "changes": r.get::<_, Option<i64>>(7)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// 実行の記録を 1 行足す。戻り値はその行の id
pub fn record(
    conn: &Connection,
    schedule_id: i64,
    now: i64,
    status: &str,
    summary: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO schedule_runs (schedule_id, started_at, finished_at, status, summary, branch)
         VALUES (?1, ?2, CASE WHEN ?3 = 'running' THEN NULL ELSE ?2 END, ?3, ?4, '')",
        params![schedule_id, now, status, summary],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 実行する予定
#[derive(Debug, Clone, PartialEq)]
pub struct Due {
    pub id: i64,
    pub name: String,
    pub dir: String,
    pub agent: String,
    pub mode: String,
    pub worktree: bool,
    pub continue_session: bool,
    pub prompt: String,
    pub last_session_id: Option<String>,
}

/// 時刻が来た予定を拾い、次の時刻を進める。遅れすぎた回は「missed」と記録して実行しない
pub fn take_due(conn: &Connection, now: DateTime<Local>) -> Result<Vec<Due>> {
    let now_ms = now.timestamp_millis();
    let mut stmt = conn.prepare(
        "SELECT id, name, dir, agent, mode, worktree, continue_session, prompt, last_session_id, repeat, next_at
         FROM schedules WHERE enabled = 1 AND next_at IS NOT NULL AND next_at <= ?1",
    )?;
    let rows = stmt
        .query_map([now_ms], |r| {
            Ok((
                Due {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    dir: r.get(2)?,
                    agent: r.get(3)?,
                    mode: r.get(4)?,
                    worktree: r.get(5)?,
                    continue_session: r.get(6)?,
                    prompt: r.get(7)?,
                    last_session_id: r.get(8)?,
                },
                r.get::<_, String>(9)?,
                r.get::<_, i64>(10)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut due = Vec::new();
    for (d, repeat, next_at) in rows {
        let repeat: Repeat = serde_json::from_str(&repeat).context("broken schedule")?;
        let next = next_after(&repeat, now).map(|t| t.timestamp_millis());
        conn.execute(
            "UPDATE schedules SET next_at = ?2, enabled = CASE WHEN ?2 IS NULL THEN 0 ELSE enabled END WHERE id = ?1",
            params![d.id, next],
        )?;
        if now_ms - next_at > LATE_MS {
            record(
                conn,
                d.id,
                now_ms,
                "missed",
                "the machine was off or asleep at the scheduled time",
            )?;
        } else {
            due.push(d);
        }
    }
    Ok(due)
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Due>> {
    Ok(conn
        .query_row(
            "SELECT id, name, dir, agent, mode, worktree, continue_session, prompt, last_session_id FROM schedules WHERE id = ?1",
            [id],
            |r| {
                Ok(Due {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    dir: r.get(2)?,
                    agent: r.get(3)?,
                    mode: r.get(4)?,
                    worktree: r.get(5)?,
                    continue_session: r.get(6)?,
                    prompt: r.get(7)?,
                    last_session_id: r.get(8)?,
                })
            },
        )
        .optional()?)
}

fn git_toplevel(dir: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["-C", dir, "rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 予定専用の作業コピー。初回に作り、以後は同じものを使う（消すのは人が決める）。戻り値は (場所, ブランチ)。
/// 置き場所は予定とリポジトリの組ごと。予定のディレクトリを別のリポジトリに変えたら新しく作り、古い方は残す
pub fn ensure_worktree(dir: &str, id: i64, base: &Path) -> Result<(String, String)> {
    let top = git_toplevel(dir).context("a worktree needs a git repository")?;
    let path = base.join(format!(
        "{id}-{}",
        &crate::share::sha256_hex(top.as_bytes())[..8]
    ));
    let branch = format!("agent-calendar/schedule-{id}");
    if !path.is_dir() {
        std::fs::create_dir_all(base)?;
        // ブランチが残っていれば（人が worktree だけ消したなど）そのまま使い、動かさない。
        // -B で作り直すと、そこに commit した成果がブランチから外れる
        let exists = Command::new("git")
            .args([
                "-C",
                &top,
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .is_ok_and(|o| o.status.success());
        let mut cmd = Command::new("git");
        cmd.args(["-C", &top, "worktree", "add"]);
        if exists {
            cmd.arg(&path).arg(&branch);
        } else {
            cmd.args(["-b", &branch]).arg(&path);
        }
        let out = cmd.output().context("cannot run git")?;
        if !out.status.success() {
            bail!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }
    Ok((path.to_string_lossy().into_owned(), branch))
}

pub fn valid_branch(name: &str) -> bool {
    !name.starts_with('-')
        && Command::new("git")
            .args(["check-ref-format", "--branch", name])
            .output()
            .is_ok_and(|o| o.status.success())
}

fn has_ref(top: &str, name: &str) -> bool {
    Command::new("git")
        .args(["-C", top, "rev-parse", "--verify", "--quiet", name])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// 新しいブランチの出発点。戻り値は (git に渡す名前, 画面に出す名前)。
/// 空なら origin の既定のブランチ（分からなければいまの HEAD）。名前を選んだときは origin のものを先に見て、
/// origin に無ければ手元のブランチ（GitHub にある状態から始めるのが基本なので）
fn start_point(top: &str, from: &str) -> Result<(String, String)> {
    if from.is_empty() {
        return Ok(match crate::github::default_branch(top) {
            Some(d) => (format!("refs/remotes/origin/{d}"), format!("origin/{d}")),
            None => ("HEAD".into(), "HEAD".into()),
        });
    }
    if !valid_branch(from) {
        bail!("not a valid branch name: {from}");
    }
    let remote = format!("refs/remotes/origin/{from}");
    if has_ref(top, &remote) {
        return Ok((remote, format!("origin/{from}")));
    }
    let local = format!("refs/heads/{from}");
    if has_ref(top, &local) {
        return Ok((local, from.to_string()));
    }
    bail!("no such branch: {from}");
}

/// 「ここで始める」で切る作業場所。`from`（空なら既定のブランチ）から新しいブランチを作り、別の作業フォルダに置く。
/// 手元の作業コピーには触らない。ブランチがすでにあれば断る（人の作業を上書きしない）。
/// 戻り値は (作業フォルダ, 出発点)
pub fn new_worktree(dir: &str, branch: &str, from: &str, base: &Path) -> Result<(String, String)> {
    let top = git_toplevel(dir).context("a worktree needs a git repository")?;
    if !valid_branch(branch) {
        bail!("not a valid branch name: {branch}");
    }
    if has_ref(&top, &format!("refs/heads/{branch}")) {
        bail!("the branch {branch} already exists");
    }
    let (start, shown) = start_point(&top, from)?;
    let path = base.join(format!(
        "{}-{}",
        &crate::share::sha256_hex(top.as_bytes())[..8],
        branch.replace('/', "-")
    ));
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    std::fs::create_dir_all(base)?;
    // origin のブランチから切っても、そこを push 先にはしない（--no-track。既定のブランチへ push させない）
    let out = Command::new("git")
        .args(["-C", &top, "worktree", "add", "--no-track", "-b", branch])
        .arg(&path)
        .arg(&start)
        .output()
        .context("cannot run git")?;
    if !out.status.success() {
        bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok((path.to_string_lossy().into_owned(), shown))
}

/// すでにある作業場所（git worktree）
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Worktree {
    pub path: String,
    /// チェックアウトしているブランチ。ブランチの上にいなければ（detached）空
    pub branch: String,
}

/// `git worktree list --porcelain` の出力から、選べる作業場所を取り出す（パスの順）。
/// 先頭は本体の作業コピーなので除く。フォルダが消えているもの（prunable）と bare も除く
fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = porcelain
        .split("\n\n")
        .skip(1)
        .filter_map(|block| {
            let mut lines = block.lines();
            let path = lines.next()?.strip_prefix("worktree ")?.to_string();
            let rest: Vec<&str> = lines.collect();
            if rest
                .iter()
                .any(|l| *l == "bare" || l.starts_with("prunable"))
            {
                return None;
            }
            let branch = rest
                .iter()
                .find_map(|l| l.strip_prefix("branch refs/heads/"))
                .unwrap_or("")
                .to_string();
            Some(Worktree { path, branch })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// そのリポジトリにすでにある作業場所（本体の作業コピーは含めない）。「ここで始める」で選ぶ。
/// agent-calendar が切ったものに限らず、git が知っているもの全部
pub fn worktrees(dir: &str) -> Vec<Worktree> {
    Command::new("git")
        .args(["-C", dir, "worktree", "list", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_worktrees(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

pub fn worktrees_dir() -> std::path::PathBuf {
    db::path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("worktrees")
}

/// 作業コピーに残っている変更の数（実行のあと、見に行くべきものがあるか）
pub fn changes(path: &str) -> Option<i64> {
    let out = Command::new("git")
        .args(["-C", path, "status", "--short"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).lines().count() as i64)
}

/// 実行の結果を記録に書く
pub fn finish(
    conn: &Connection,
    run_id: i64,
    ok: bool,
    session: Option<&str>,
    summary: &str,
    branch: &str,
    changes: Option<i64>,
) -> Result<()> {
    conn.execute(
        "UPDATE schedule_runs SET finished_at = ?2, status = ?3, session_id = ?4, summary = ?5, branch = ?6, changes = ?7 WHERE id = ?1",
        params![run_id, chrono::Utc::now().timestamp_millis(), if ok { "ok" } else { "failed" }, session, summary, branch, changes],
    )?;
    Ok(())
}

/// 実行しなかった・起動できなかった回の記録を閉じる
pub fn mark(conn: &Connection, run_id: i64, status: &str, summary: &str) -> Result<()> {
    conn.execute(
        "UPDATE schedule_runs SET finished_at = ?2, status = ?3, summary = ?4 WHERE id = ?1",
        params![
            run_id,
            chrono::Utc::now().timestamp_millis(),
            status,
            summary
        ],
    )?;
    Ok(())
}

/// 予定を 1 回起動する（tokio の中から呼ぶ）。終わるのを待たずに返り、結果は実行の記録に書く。
/// `after` は終わったあとに 1 回呼ぶ（日誌の読み直し）
pub fn launch(
    running: &send::Running,
    d: &Due,
    after: impl FnOnce() + Send + 'static,
) -> Result<i64> {
    let conn = db::open()?;
    let run_id = record(
        &conn,
        d.id,
        chrono::Utc::now().timestamp_millis(),
        "running",
        "",
    )?;
    let fail = |status: &str, msg: String| -> Result<i64> {
        mark(&conn, run_id, status, &msg)?;
        Ok(run_id)
    };
    let (cwd, branch) = if d.worktree {
        match ensure_worktree(&d.dir, d.id, &worktrees_dir()) {
            Ok(v) => v,
            Err(e) => return fail("failed", format!("{e:#}")),
        }
    } else {
        (d.dir.clone(), String::new())
    };
    let mode: send::Mode = serde_json::from_value(json!(d.mode)).unwrap_or(send::Mode::Read);
    // 同じ予定は重ねて動かさない（前の回が長引いたら、この回は飛ばす）
    let Some(schedule_claim) = running.claim(&format!("schedule:{}", d.id)) else {
        return fail(
            "skipped",
            "the previous run of this schedule is still running".into(),
        );
    };
    let continuing = d.last_session_id.as_deref().filter(|_| d.continue_session);
    let (program, args) = match continuing {
        Some(sid) => send::command(&d.agent, sid, mode),
        None => send::command_new(&d.agent, mode, ""),
    };
    let job = send::Job {
        program: program.to_string(),
        args,
        source: d.agent.clone(),
        // 前回に続けるときはセッション ID で印を付ける。画面から同じセッションへ送っている最中なら重ならない
        id: continuing
            .map(str::to_string)
            .unwrap_or_else(|| format!("schedule-new:{}", d.id)),
        cwd: cwd.clone(),
        prompt: format!("{}\n", d.prompt),
        timeout: send::TIMEOUT,
        cleanup: Vec::new(),
        // 予定は無人で動くので、許可の問い合わせには答える人がいない（出たものは断られる）
        ask: false,
    };
    let mut rx = match send::start(running, job, after) {
        Ok(rx) => rx,
        Err(e) => return fail("skipped", format!("{e:#}")),
    };
    let (id, worktree) = (d.id, d.worktree);
    tokio::spawn(async move {
        let _schedule_claim = schedule_claim;
        let (mut session, mut last_text, mut ok, mut error) =
            (None::<String>, String::new(), false, String::new());
        while let Some(line) = rx.recv().await {
            let Ok(ev) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            match ev["kind"].as_str() {
                Some("session") => session = ev["id"].as_str().map(str::to_string),
                Some("text") => last_text = ev["text"].as_str().unwrap_or("").to_string(),
                Some("done") => {
                    ok = ev["ok"] == true;
                    error = ev["text"].as_str().unwrap_or("").to_string();
                }
                _ => {}
            }
        }
        let summary = if ok { last_text } else { error };
        let changes = if worktree { changes(&cwd) } else { None };
        let _ = tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = db::open()?;
            finish(
                &conn,
                run_id,
                ok,
                session.as_deref(),
                &crate::transcript::clip(&summary),
                &branch,
                changes,
            )?;
            if let Some(s) = session {
                remember_session(&conn, id, &s)?;
            }
            Ok(())
        })
        .await;
    });
    Ok(run_id)
}

/// サービスが止まると、実行中の回の終わりを書く処理も消える。起動時に、残った「実行中」を中断として閉じる
pub fn recover(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE schedule_runs SET status = 'failed', finished_at = ?1,
         summary = 'interrupted: the service stopped while this was running' WHERE status = 'running'",
        [chrono::Utc::now().timestamp_millis()],
    )?)
}

pub fn remember_session(conn: &Connection, id: i64, session: &str) -> Result<()> {
    conn.execute(
        "UPDATE schedules SET last_session_id = ?2 WHERE id = ?1",
        params![id, session],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Local> {
        let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        Local.from_local_datetime(&naive).earliest().unwrap()
    }

    const WEEKDAYS: u8 = 0b001_1111;
    const ALL: u8 = 0b111_1111;

    #[test]
    fn next_once() {
        let r = Repeat::Once {
            at: t("2026-10-05 09:00").timestamp_millis(),
        };
        assert_eq!(
            next_after(&r, t("2026-10-04 12:00")),
            Some(t("2026-10-05 09:00"))
        );
        assert_eq!(next_after(&r, t("2026-10-05 09:00")), None);
    }

    #[test]
    fn next_weekly() {
        // 2026-10-04 は日曜
        let r = Repeat::Weekly {
            days: WEEKDAYS,
            minute: 8 * 60,
        };
        assert_eq!(
            next_after(&r, t("2026-10-04 12:00")),
            Some(t("2026-10-05 08:00"))
        );
        // 今日の時刻がまだ先なら今日
        assert_eq!(
            next_after(&r, t("2026-10-05 07:59")),
            Some(t("2026-10-05 08:00"))
        );
        assert_eq!(
            next_after(&r, t("2026-10-05 08:00")),
            Some(t("2026-10-06 08:00"))
        );
        // 金曜のあとは月曜
        assert_eq!(
            next_after(&r, t("2026-10-09 09:00")),
            Some(t("2026-10-12 08:00"))
        );
        let monday = Repeat::Weekly {
            days: 1,
            minute: 9 * 60 + 30,
        };
        assert_eq!(
            next_after(&monday, t("2026-10-05 10:00")),
            Some(t("2026-10-12 09:30"))
        );
    }

    #[test]
    fn next_interval() {
        let r = Repeat::Interval {
            days: ALL,
            every_min: 120,
            from_hour: 7,
            to_hour: 22,
        };
        assert_eq!(
            next_after(&r, t("2026-10-04 06:00")),
            Some(t("2026-10-04 07:00"))
        );
        assert_eq!(
            next_after(&r, t("2026-10-04 07:00")),
            Some(t("2026-10-04 09:00"))
        );
        // 21:00 が最後（23:00 は 22 時を超える）。その次は翌朝 7:00
        assert_eq!(
            next_after(&r, t("2026-10-04 20:00")),
            Some(t("2026-10-04 21:00"))
        );
        assert_eq!(
            next_after(&r, t("2026-10-04 21:00")),
            Some(t("2026-10-05 07:00"))
        );
        let weekdays = Repeat::Interval {
            days: WEEKDAYS,
            every_min: 60,
            from_hour: 0,
            to_hour: 24,
        };
        assert_eq!(
            next_after(&weekdays, t("2026-10-04 12:00")),
            Some(t("2026-10-05 00:00"))
        );
        assert_eq!(
            next_after(&weekdays, t("2026-10-09 23:00")),
            Some(t("2026-10-12 00:00"))
        );
    }

    #[test]
    fn occurrences_in_range() {
        // 2026-10-05 は月曜
        let once = Repeat::Once {
            at: t("2026-10-06 09:00").timestamp_millis(),
        };
        let from = t("2026-10-05 00:00");
        let to = t("2026-10-12 00:00");
        assert_eq!(
            occurrences(&once, from, to),
            vec![(t("2026-10-06 09:00"), None)]
        );
        assert!(occurrences(&once, t("2026-10-06 09:01"), to).is_empty());
        assert!(occurrences(&once, from, t("2026-10-06 09:00")).is_empty());

        let weekdays = Repeat::Weekly {
            days: WEEKDAYS,
            minute: 8 * 60,
        };
        let w = occurrences(&weekdays, t("2026-10-05 08:00"), to);
        assert_eq!(w.len(), 5); // 開始ちょうどの回も含む
        assert_eq!(w[0], (t("2026-10-05 08:00"), None));
        assert_eq!(w[4], (t("2026-10-09 08:00"), None));

        let interval = Repeat::Interval {
            days: 0b000_0011, // 月・火
            every_min: 30,
            from_hour: 9,
            to_hour: 18,
        };
        // 月曜の途中からなら、その日の帯は次の回から始まる
        assert_eq!(
            occurrences(&interval, t("2026-10-05 12:10"), to),
            vec![
                (t("2026-10-05 12:30"), Some(t("2026-10-05 18:00"))),
                (t("2026-10-06 09:00"), Some(t("2026-10-06 18:00"))),
            ]
        );
        let all_day = Repeat::Interval {
            days: ALL,
            every_min: 60,
            from_hour: 0,
            to_hour: 24,
        };
        assert_eq!(
            occurrences(&all_day, from, t("2026-10-07 00:00")),
            vec![
                (t("2026-10-05 00:00"), Some(t("2026-10-06 00:00"))),
                (t("2026-10-06 00:00"), Some(t("2026-10-07 00:00"))),
            ]
        );
    }

    #[test]
    fn upcoming_skips_disabled() {
        let dir = db::temp_dir("sched-upcoming");
        let conn = db::open_at(&dir.join("t.db")).unwrap();
        let d = dir.to_str().unwrap();
        let now = t("2026-10-04 12:00");
        let id = save(&conn, None, &input(d), now).unwrap();
        let mut off = input(d);
        off.enabled = false;
        save(&conn, None, &off, now).unwrap();
        let mut interval = input(d);
        interval.repeat = Repeat::Interval {
            days: ALL,
            every_min: 60,
            from_hour: 9,
            to_hour: 18,
        };
        let iid = save(&conn, None, &interval, now).unwrap();
        let u = upcoming(&conn, now, t("2026-10-06 00:00")).unwrap();
        assert_eq!(u.len(), 3);
        assert_eq!(u[0]["id"], id);
        assert_eq!(u[0]["start"], t("2026-10-05 08:00").timestamp_millis());
        assert_eq!(u[0]["end"], Value::Null);
        assert_eq!(u[0]["every_min"], Value::Null);
        assert_eq!(u[1]["id"], iid);
        // 開始ちょうど（12:00）の回も入る
        assert_eq!(u[1]["start"], t("2026-10-04 12:00").timestamp_millis());
        assert_eq!(u[1]["end"], t("2026-10-04 18:00").timestamp_millis());
        assert_eq!(u[1]["every_min"], 60);
        assert_eq!(u[2]["start"], t("2026-10-05 09:00").timestamp_millis());
        assert_eq!(u[2]["name"], "朝のテスト");
    }

    #[test]
    fn unfinished_runs_stay_until_ok() {
        let dir = db::temp_dir("sched-unfinished");
        let conn = db::open_at(&dir.join("t.db")).unwrap();
        let id = save(
            &conn,
            None,
            &input(dir.to_str().unwrap()),
            t("2026-10-04 12:00"),
        )
        .unwrap();
        let running = record(&conn, id, 1000, "running", "").unwrap();
        record(&conn, id, 2000, "missed", "").unwrap();
        let failed = record(&conn, id, 3000, "running", "").unwrap();
        finish(&conn, failed, false, Some("s0"), "boom", "", None).unwrap();
        // 正常に終わった回（次の予定時刻より後なので、上の失敗のやり直しには数えない）
        let later = 2 * 24 * 3600 * 1000;
        let ok = record(&conn, id, later, "running", "").unwrap();
        finish(&conn, ok, true, Some("s1"), "", "", None).unwrap();
        let r = unfinished_runs(&conn, 0, later + 1).unwrap();
        let got: Vec<_> = r
            .iter()
            .map(|x| (x["start"].as_i64().unwrap(), x["status"].as_str().unwrap()))
            .collect();
        assert_eq!(
            got,
            vec![(1000, "running"), (2000, "missed"), (3000, "failed")]
        );
        assert_eq!(r[0]["name"], "朝のテスト");
        // 範囲の外は出さない
        assert!(unfinished_runs(&conn, 3500, 5000).unwrap().is_empty());
        // 正常に終わったら消える
        finish(&conn, running, true, Some("s2"), "", "", None).unwrap();
        assert_eq!(unfinished_runs(&conn, 0, 1500).unwrap().len(), 0);
    }

    #[test]
    fn redone_runs_hide_the_failure() {
        let dir = db::temp_dir("sched-redone");
        let conn = db::open_at(&dir.join("t.db")).unwrap();
        let d = dir.to_str().unwrap();
        let ms = |s: &str| t(s).timestamp_millis();
        let fail_at = |id: i64, at: &str| {
            let r = record(&conn, id, ms(at), "running", "").unwrap();
            finish(&conn, r, false, None, "interrupted", "", None).unwrap();
        };
        let ok_at = |id: i64, at: &str| {
            let r = record(&conn, id, ms(at), "running", "").unwrap();
            finish(&conn, r, true, Some("s"), "", "", None).unwrap();
        };
        let shown = |conn: &Connection| unfinished_runs(conn, 0, i64::MAX).unwrap().len();
        // 1 回だけの予定: 失敗のあと、やり直しが成功したら消える
        let mut once = input(d);
        once.repeat = Repeat::Once {
            at: ms("2026-10-04 13:00"),
        };
        let oid = save(&conn, None, &once, t("2026-10-04 12:00")).unwrap();
        fail_at(oid, "2026-10-04 13:00");
        assert_eq!(shown(&conn), 1);
        // 時刻を 13:06 に変えてやり直した（次の時刻がやり直しの回そのもの）
        let mut moved = once.clone();
        moved.repeat = Repeat::Once {
            at: ms("2026-10-04 13:06"),
        };
        save(&conn, Some(oid), &moved, t("2026-10-04 13:01")).unwrap();
        ok_at(oid, "2026-10-04 13:06");
        assert_eq!(shown(&conn), 0);
        // 毎日 8:00 の予定: 次の回（翌日 8:00）より前のやり直しだけが失敗を消す
        let wid = save(&conn, None, &input(d), t("2026-10-04 12:00")).unwrap();
        fail_at(wid, "2026-10-05 08:00");
        ok_at(wid, "2026-10-06 08:00");
        assert_eq!(shown(&conn), 1);
        ok_at(wid, "2026-10-05 09:00");
        assert_eq!(shown(&conn), 0);
        // 前の回が動いていて飛ばした回は、飛ばした時点で動いていた回が正常に終われば消える。
        // finish は終わった時刻を今にするので、ここでは終わった時刻を直接入れる
        let sid = save(&conn, None, &input(d), t("2026-10-04 12:00")).unwrap();
        let run = |at: &str, end: Option<&str>, status: &str| {
            conn.execute(
                "INSERT INTO schedule_runs (schedule_id, started_at, finished_at, status, summary, branch) VALUES (?1, ?2, ?3, ?4, '', '')",
                params![sid, ms(at), end.map(ms), status],
            )
            .unwrap();
        };
        run("2026-10-07 07:50", None, "running");
        run("2026-10-07 08:00", Some("2026-10-07 08:00"), "skipped");
        assert_eq!(shown(&conn), 2); // 実行中の前の回と、飛ばした回
        conn.execute(
            "UPDATE schedule_runs SET status = 'ok', finished_at = ?1 WHERE status = 'running'",
            [ms("2026-10-07 08:05")],
        )
        .unwrap();
        assert_eq!(shown(&conn), 0);
        // 飛ばした時点より前に終わっていた成功（無関係な手動の実行）では消えない
        run("2026-10-08 07:55", Some("2026-10-08 07:57"), "ok");
        run("2026-10-08 07:58", Some("2026-10-08 08:20"), "failed");
        run("2026-10-08 08:00", Some("2026-10-08 08:00"), "skipped");
        assert_eq!(shown(&conn), 2); // 失敗した回と、飛ばした回
    }

    fn input(dir: &str) -> Input {
        Input {
            name: "朝のテスト".into(),
            dir: dir.into(),
            agent: "claude".into(),
            mode: send::Mode::Read,
            worktree: false,
            continue_session: false,
            prompt: "cargo test を流して".into(),
            repeat: Repeat::Weekly {
                days: ALL,
                minute: 8 * 60,
            },
            enabled: true,
        }
    }

    #[test]
    fn validates_input() {
        let dir = db::temp_dir("sched-valid");
        let d = dir.to_str().unwrap();
        let now = t("2026-10-04 12:00");
        assert!(validate(&input(d), now).is_ok());
        let bad = |f: &dyn Fn(&mut Input)| {
            let mut i = input(d);
            f(&mut i);
            validate(&i, now).is_err()
        };
        assert!(bad(&|i| i.name = " ".into()));
        assert!(bad(&|i| i.dir = "/nonexistent".into()));
        assert!(bad(&|i| i.agent = "gpt".into()));
        assert!(bad(&|i| i.prompt = String::new()));
        assert!(bad(&|i| i.worktree = true)); // git ではない
        assert!(bad(&|i| i.repeat = Repeat::Once {
            at: now.timestamp_millis()
        }));
        assert!(bad(&|i| i.repeat = Repeat::Weekly {
            days: 0,
            minute: 60
        }));
        assert!(bad(&|i| i.repeat = Repeat::Interval {
            days: ALL,
            every_min: 10,
            from_hour: 7,
            to_hour: 22
        }));
        assert!(bad(&|i| i.repeat = Repeat::Interval {
            days: ALL,
            every_min: 60,
            from_hour: 22,
            to_hour: 7
        }));
    }

    #[test]
    fn saves_takes_due_and_records_missed() {
        let dir = db::temp_dir("sched-due");
        let conn = db::open_at(&dir.join("t.db")).unwrap();
        let d = dir.to_str().unwrap();
        let id = save(&conn, None, &input(d), t("2026-10-04 12:00")).unwrap();
        let next = |conn: &Connection| {
            conn.query_row("SELECT next_at FROM schedules WHERE id = ?1", [id], |r| {
                r.get::<_, Option<i64>>(0)
            })
            .unwrap()
        };
        assert_eq!(next(&conn), Some(t("2026-10-05 08:00").timestamp_millis()));
        // まだ早い
        assert!(take_due(&conn, t("2026-10-05 07:59")).unwrap().is_empty());
        // 時刻が来たら拾い、次の回へ進める
        let due = take_due(&conn, t("2026-10-05 08:01")).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].prompt, "cargo test を流して");
        assert_eq!(next(&conn), Some(t("2026-10-06 08:00").timestamp_millis()));
        // PC が止まっていた（翌々日に起動）→ 実行せず missed を残す
        assert!(take_due(&conn, t("2026-10-07 12:00")).unwrap().is_empty());
        let r = runs(&conn, id, 5).unwrap();
        assert_eq!(r[0]["status"], "missed");
        assert_eq!(next(&conn), Some(t("2026-10-08 08:00").timestamp_millis()));
        // 1 回だけの予定は動いたら止まる
        let mut once = input(d);
        once.repeat = Repeat::Once {
            at: t("2026-10-08 10:00").timestamp_millis(),
        };
        let oid = save(&conn, None, &once, t("2026-10-08 09:00")).unwrap();
        assert_eq!(
            take_due(&conn, t("2026-10-08 10:00"))
                .unwrap()
                .iter()
                .map(|d| d.id)
                .collect::<Vec<_>>(),
            [oid]
        );
        let (enabled, n): (bool, Option<i64>) = conn
            .query_row(
                "SELECT enabled, next_at FROM schedules WHERE id = ?1",
                [oid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((enabled, n), (false, None));
        // 更新・無効・削除
        let mut off = input(d);
        off.enabled = false;
        save(&conn, Some(id), &off, t("2026-10-08 09:00")).unwrap();
        assert_eq!(next(&conn), None);
        assert!(save(&conn, Some(999), &off, t("2026-10-08 09:00")).is_err());
        let l = list(&conn).unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0]["repeat"]["kind"], "weekly");
        assert_eq!(get(&conn, id).unwrap().unwrap().name, "朝のテスト");
        assert!(delete(&conn, id).unwrap());
        assert!(!delete(&conn, id).unwrap());
        assert!(get(&conn, id).unwrap().is_none());
    }

    #[test]
    fn records_runs_and_sessions() {
        let dir = db::temp_dir("sched-runs");
        let conn = db::open_at(&dir.join("t.db")).unwrap();
        let id = save(
            &conn,
            None,
            &input(dir.to_str().unwrap()),
            t("2026-10-04 12:00"),
        )
        .unwrap();
        let run = record(&conn, id, 1000, "running", "").unwrap();
        finish(
            &conn,
            run,
            true,
            Some("s1"),
            "done",
            "agent-calendar/schedule-1",
            Some(2),
        )
        .unwrap();
        remember_session(&conn, id, "s1").unwrap();
        let r = runs(&conn, id, 5).unwrap();
        assert_eq!(
            (
                r[0]["status"].as_str(),
                r[0]["session_id"].as_str(),
                r[0]["changes"].as_i64()
            ),
            (Some("ok"), Some("s1"), Some(2))
        );
        assert_eq!(
            get(&conn, id).unwrap().unwrap().last_session_id.as_deref(),
            Some("s1")
        );
        // 名前を変えるだけなら続けられる。エージェントを変えたら前回のセッションは捨てる
        let mut renamed = input(dir.to_str().unwrap());
        renamed.name = "別名".into();
        save(&conn, Some(id), &renamed, t("2026-10-04 12:00")).unwrap();
        assert_eq!(
            get(&conn, id).unwrap().unwrap().last_session_id.as_deref(),
            Some("s1")
        );
        renamed.agent = "codex".into();
        save(&conn, Some(id), &renamed, t("2026-10-04 12:00")).unwrap();
        assert_eq!(get(&conn, id).unwrap().unwrap().last_session_id, None);
        // 再起動で残った「実行中」は中断として閉じる
        let stuck = record(&conn, id, 2000, "running", "").unwrap();
        assert_eq!(recover(&conn).unwrap(), 1);
        let r = runs(&conn, id, 5).unwrap();
        let row = r.iter().find(|x| x["id"] == stuck).unwrap();
        assert_eq!(row["status"], "failed");
        assert!(row["finished_at"].is_i64());
    }

    #[test]
    fn worktree_is_created_once() {
        let dir = db::temp_dir("sched-wt");
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q", "-b", "master"]);
        git(&[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        // 「ここで始める」の作業場所: 新しいブランチで切る。同じブランチ・おかしな名前は断る
        let wbase = dir.join("start-wts");
        let r = repo.to_str().unwrap();
        // origin/HEAD が分からないうちは、いまの HEAD から切る
        let (ws, from) = new_worktree(r, "feature/try-1", "", &wbase).unwrap();
        assert!(ws.ends_with("-feature-try-1"));
        assert_eq!(from, "HEAD");
        assert!(Path::new(&ws).join(".git").exists());
        assert!(new_worktree(r, "feature/try-1", "", &wbase).is_err());
        assert!(new_worktree(r, "bad..name", "", &wbase).is_err());
        assert!(new_worktree(r, "-x", "", &wbase).is_err());
        assert!(new_worktree(dir.to_str().unwrap(), "y", "", &wbase).is_err()); // git ではない
        // 出発点: origin の既定のブランチ（1 つ前のコミット）と、手元だけのブランチ（その先のコミット）を用意する
        let rev = |dir: &str, name: &str| {
            let o = Command::new("git")
                .args(["-C", dir, "rev-parse", name])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        git(&["remote", "add", "origin", "https://github.com/x/y.git"]);
        git(&["config", "branch.autoSetupMerge", "true"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        git(&["update-ref", "refs/remotes/origin/release", "HEAD"]);
        git(&[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "y",
        ]);
        git(&["branch", "local-only"]);
        git(&["branch", "release"]); // 手元の release は origin より先に進んでいる
        let origin_main = rev(r, "refs/remotes/origin/main");
        assert_ne!(origin_main, rev(r, "HEAD"));
        // 空なら既定のブランチから（いまの HEAD からではない）。そこを push 先にはしない
        let (ws, from) = new_worktree(r, "feature/try-2", "", &wbase).unwrap();
        assert_eq!(from, "origin/main");
        assert_eq!(rev(&ws, "HEAD"), origin_main);
        assert!(!has_ref(r, "feature/try-2@{upstream}"));
        // 選んだブランチから。origin と手元の両方にあれば origin、origin に無ければ手元
        let (ws, from) = new_worktree(r, "feature/try-3", "release", &wbase).unwrap();
        assert_eq!(
            (from.as_str(), rev(&ws, "HEAD")),
            ("origin/release", origin_main)
        );
        let (ws, from) = new_worktree(r, "feature/try-4", "local-only", &wbase).unwrap();
        assert_eq!(from, "local-only");
        assert_eq!(rev(&ws, "HEAD"), rev(r, "HEAD"));
        // すでにある作業場所の一覧: 切ったものが出て、本体の作業コピーは出ない
        let listed = worktrees(r);
        assert_eq!(
            listed.iter().map(|w| w.branch.as_str()).collect::<Vec<_>>(),
            [
                "feature/try-1",
                "feature/try-2",
                "feature/try-3",
                "feature/try-4"
            ]
        );
        assert_eq!(listed[3].path, ws);
        assert!(worktrees("/nonexistent").is_empty());
        assert_eq!(
            parse_worktrees(
                "worktree /r/main\nHEAD a\nbranch refs/heads/main\n\n\
                 worktree /w/z\nHEAD b\nbranch refs/heads/feature/z\n\n\
                 worktree /w/d\nHEAD c\ndetached\n\n\
                 worktree /w/gone\nHEAD d\nbranch refs/heads/gone\nprunable gitdir file points to non-existent location\n\n\
                 worktree /w/bare\nbare\n"
            ),
            [
                Worktree {
                    path: "/w/d".into(),
                    branch: String::new()
                },
                Worktree {
                    path: "/w/z".into(),
                    branch: "feature/z".into()
                },
            ]
        );
        // 無いブランチ・ブランチ名でないものは断る
        assert!(new_worktree(r, "feature/try-5", "nope", &wbase).is_err());
        assert!(new_worktree(r, "feature/try-5", "main~1", &wbase).is_err());
        assert!(new_worktree(r, "feature/try-5", "-x", &wbase).is_err());
        assert_eq!(
            (
                crate::github::default_branch(r).as_deref(),
                crate::github::branches(r)
            ),
            (
                Some("main"),
                [
                    "feature/try-1",
                    "feature/try-2",
                    "feature/try-3",
                    "feature/try-4",
                    "local-only",
                    "main",
                    "master",
                    "release"
                ]
                .map(String::from)
                .to_vec()
            )
        );
        let base = dir.join("wts");
        let (path, branch) = ensure_worktree(repo.to_str().unwrap(), 7, &base).unwrap();
        assert_eq!(branch, "agent-calendar/schedule-7");
        assert!(Path::new(&path).join(".git").exists());
        assert_eq!(
            ensure_worktree(repo.to_str().unwrap(), 7, &base).unwrap().0,
            path
        );
        std::fs::write(Path::new(&path).join("new.txt"), "x").unwrap();
        assert_eq!(changes(&path), Some(1));
        // 作業コピーに commit したあと、人が作業コピーだけ消しても、作り直しでブランチの成果を失わない
        let wt = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&path)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        wt(&["add", "."]);
        wt(&[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-q",
            "-m",
            "work",
        ]);
        git(&["worktree", "remove", "--force", &path]);
        let again = ensure_worktree(repo.to_str().unwrap(), 7, &base).unwrap().0;
        assert!(Path::new(&again).join("new.txt").exists());
        // 別のリポジトリに変えたら別の作業コピー
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let og = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&other)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        og(&["init", "-q"]);
        og(&[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        assert_ne!(
            ensure_worktree(other.to_str().unwrap(), 7, &base)
                .unwrap()
                .0,
            again
        );
        assert!(ensure_worktree(dir.to_str().unwrap(), 8, &base).is_err());
        assert!(worktrees_dir().ends_with("agent-calendar/worktrees"));
    }
}
