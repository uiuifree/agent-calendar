//! 各エージェントの利用枠（5 時間・週・モデルごと）の使い具合と、戻る時刻。外へは問い合わせず、手元に残っているものだけ読む。
//! - Codex: セッションの記録（rollout）の token_count に rate_limits が入っている
//! - Claude Code・Antigravity（agy）: statusline のコマンドに渡される JSON にしか無いので、
//!   `agent-calendar statusline --agent claude|gemini` を statusline に登録してもらい、受け取った分をファイルに残す
//! - agy・Muse: 上限に当たったときの記録（会話の DB・セッションの記録）から、どの枠がいつ戻るか
//!
//! どれも「最後に動いたときの値」なので、いつの時点かを一緒に返す。戻る時刻を過ぎた枠は出さない
use crate::{codex, db};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// statusline から受け取った分を置くフォルダ
pub fn dir() -> PathBuf {
    db::path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("limits")
}

/// 1 つの枠
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// five_hour / seven_day / window（それ以外の長さ）/ model（モデルごと）/ quota（agy の枠の名前）
    pub kind: String,
    /// 枠の名前やモデル名（kind が model・quota のとき）
    pub label: String,
    pub used_percent: Option<f64>,
    /// 戻る時刻（unix ms）
    pub resets_at: Option<i64>,
    /// 使い切っている（上限に当たった記録から）
    pub exhausted: bool,
}

impl Window {
    fn to_json(&self) -> Value {
        json!({
            "kind": self.kind, "label": self.label, "used_percent": self.used_percent,
            "resets_at": self.resets_at, "exhausted": self.exhausted,
        })
    }
}

/// 1 つのエージェントの分
#[derive(Debug, Default, PartialEq)]
pub struct Agent {
    pub windows: Vec<Window>,
    /// いつの時点の値か（unix ms）
    pub observed_at: Option<i64>,
    pub plan: Option<String>,
}

/// 画面に出す形。setup は値が無いときの取り方の案内（statusline を登録する）
pub fn all(home: &Path, saved: &Path, now_ms: i64) -> Value {
    let agents = [
        ("claude", read_statusline(saved, "claude"), true),
        ("codex", codex_limits(&home.join(".codex/sessions")), false),
        (
            "gemini",
            merge(
                read_statusline(saved, "gemini"),
                gemini_exhausted(&home.join(".gemini/antigravity-cli/conversations")),
            ),
            true,
        ),
        (
            "muse",
            muse_exhausted(&home.join(".local/share/muse/sessions"), now_ms),
            false,
        ),
    ];
    let list: Vec<Value> = agents
        .into_iter()
        .map(|(name, a, needs_statusline)| {
            let a = a.unwrap_or_default();
            // 戻る時刻を過ぎた枠はもう当てにならないので出さず、数だけ返す（「最後の記録のあとで戻った」と出せるように）
            let (live, passed): (Vec<&Window>, Vec<&Window>) = a
                .windows
                .iter()
                .partition(|w| w.resets_at.is_none_or(|t| t > now_ms));
            let windows: Vec<Value> = live.into_iter().map(Window::to_json).collect();
            let setup = (needs_statusline && a.observed_at.is_none()).then_some("statusline");
            json!({
                "agent": name, "windows": windows, "observed_at": a.observed_at, "plan": a.plan,
                "setup": setup, "reset": passed.len(),
            })
        })
        .collect();
    json!({ "agents": list, "now": now_ms })
}

fn merge(a: Option<Agent>, b: Option<Agent>) -> Option<Agent> {
    match (a, b) {
        (Some(mut a), Some(b)) => {
            a.windows.extend(b.windows);
            // いつの時点かは、新しいほう
            a.observed_at = a.observed_at.max(b.observed_at);
            Some(a)
        }
        (a, b) => a.or(b),
    }
}

/// 枠の長さ（分）から名前を決める
fn kind_of(minutes: i64) -> &'static str {
    match minutes {
        300 => "five_hour",
        10080 => "seven_day",
        _ => "window",
    }
}

// ---- Codex ----

/// 新しい記録から順に見て、最後の rate_limits を取る（無ければ次に新しい記録へ。見るのは 5 つまで）
pub fn codex_limits(root: &Path) -> Option<Agent> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = codex::session_files(root)
        .ok()?
        .into_iter()
        .filter_map(|f| Some((std::fs::metadata(&f).ok()?.modified().ok()?, f)))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
        .into_iter()
        .take(5)
        .find_map(|(_, f)| codex_from(&std::fs::read_to_string(f).ok()?))
}

fn codex_from(text: &str) -> Option<Agent> {
    text.lines().rev().find_map(|line| {
        if !line.contains("\"rate_limits\"") {
            return None;
        }
        let v: Value = serde_json::from_str(line).ok()?;
        let rl = &v["payload"]["rate_limits"];
        if !rl.is_object() {
            return None;
        }
        let windows = ["primary", "secondary"]
            .iter()
            .filter_map(|k| {
                let w = &rl[k];
                let minutes = w["window_minutes"].as_i64()?;
                Some(Window {
                    kind: kind_of(minutes).into(),
                    label: if kind_of(minutes) == "window" {
                        format!("{minutes}m")
                    } else {
                        String::new()
                    },
                    used_percent: w["used_percent"].as_f64(),
                    resets_at: w["resets_at"].as_i64().map(|s| s * 1000),
                    exhausted: w["used_percent"].as_f64().is_some_and(|p| p >= 100.0),
                })
            })
            .collect();
        Some(Agent {
            windows,
            observed_at: v["timestamp"].as_str().and_then(crate::scan::parse_ts),
            plan: rl["plan_type"].as_str().map(str::to_string),
        })
    })
}

// ---- statusline（Claude Code・agy）----

/// statusline のコマンドが受け取った JSON から、枠のところだけ取り出して残す。戻り値は端末に出す短い文
pub fn save_statusline(dir: &Path, agent: &str, input: &str, now_ms: i64) -> Result<String> {
    let v: Value = serde_json::from_str(input).context("the status line input is not JSON")?;
    let keep = json!({
        "observed_at": now_ms,
        "rate_limits": v["rate_limits"],
        "quota": v["quota"],
        "plan_tier": v["plan_tier"],
    });
    let line = short(&agent_from_statusline(&keep));
    // 枠の値が無い回（セッションの最初の応答の前など）は、前に残した値を消さない
    if v["rate_limits"].is_object() || v["quota"].is_object() {
        std::fs::create_dir_all(dir)?;
        let file = dir.join(format!("{agent}.json"));
        let tmp = dir.join(format!(".{agent}.json.{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(&keep)?)?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &file)?;
    }
    Ok(line)
}

fn read_statusline(dir: &Path, agent: &str) -> Option<Agent> {
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{agent}.json"))).ok()?)
            .ok()?;
    Some(agent_from_statusline(&v))
}

fn agent_from_statusline(v: &Value) -> Agent {
    let mut windows = Vec::new();
    // Claude Code: rate_limits.{five_hour, seven_day}.{used_percentage, resets_at（秒）}
    for (k, w) in v["rate_limits"].as_object().into_iter().flatten() {
        if !w.is_object() {
            continue;
        }
        windows.push(Window {
            kind: match k.as_str() {
                "five_hour" | "seven_day" => k.clone(),
                _ => "quota".into(),
            },
            label: if k == "five_hour" || k == "seven_day" {
                String::new()
            } else {
                k.clone()
            },
            used_percent: w["used_percentage"].as_f64(),
            resets_at: w["resets_at"].as_i64().map(|s| s * 1000),
            exhausted: w["used_percentage"].as_f64().is_some_and(|p| p >= 100.0),
        });
    }
    // agy: quota.<枠>.{remaining_fraction, reset_time（ISO）}
    for (k, q) in v["quota"].as_object().into_iter().flatten() {
        let used = q["remaining_fraction"]
            .as_f64()
            .map(|f| ((1.0 - f) * 1000.0).round() / 10.0);
        windows.push(Window {
            kind: "quota".into(),
            label: k.clone(),
            used_percent: used,
            resets_at: q["reset_time"].as_str().and_then(crate::scan::parse_ts),
            exhausted: used.is_some_and(|p| p >= 100.0),
        });
    }
    Agent {
        windows,
        observed_at: v["observed_at"].as_i64(),
        plan: v["plan_tier"].as_str().map(str::to_string),
    }
}

/// 端末の statusline に出す短い文（例: `5h 23% · 7d 41%`）
fn short(a: &Agent) -> String {
    a.windows
        .iter()
        .filter_map(|w| {
            let p = w.used_percent?;
            let name = match w.kind.as_str() {
                "five_hour" => "5h".to_string(),
                "seven_day" => "7d".to_string(),
                _ => w.label.clone(),
            };
            Some(format!("{name} {p:.0}%"))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

// ---- 上限に当たった記録（agy・Muse）----

/// agy の会話の DB（新しいもの 20 個まで）から、上限に当たったときのエラー（QUOTA_EXHAUSTED）を拾う。
/// モデルごとに一番新しいもの
pub fn gemini_exhausted(dir: &Path) -> Option<Agent> {
    let mut dbs: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "db"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    dbs.sort_by_key(|d| std::cmp::Reverse(d.0));
    let mut found: Vec<(String, i64, i64)> = Vec::new(); // (モデル, 戻る時刻, 当たった時刻)
    for (modified, db) in dbs.into_iter().take(20) {
        let at = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        // 同じモデルが何度も当たっていれば、一番遅く戻るもの（古い会話の、もう戻った記録で隠さない）
        for (model, reset) in quota_errors(&db) {
            match found.iter_mut().find(|(m, _, _)| *m == model) {
                Some(f) if reset > f.1 => *f = (model, reset, at),
                Some(_) => {}
                None => found.push((model, reset, at)),
            }
        }
    }
    if found.is_empty() {
        return None;
    }
    Some(Agent {
        observed_at: found.iter().map(|f| f.2).max(),
        windows: found
            .into_iter()
            .map(|(model, reset, _)| Window {
                kind: "model".into(),
                label: model,
                used_percent: Some(100.0),
                resets_at: Some(reset),
                exhausted: true,
            })
            .collect(),
        plan: None,
    })
}

fn quota_errors(db: &Path) -> Vec<(String, i64)> {
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT step_payload FROM steps WHERE step_type = 17 ORDER BY idx DESC")
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0)) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|b| error_info(&String::from_utf8_lossy(&b)))
        .collect()
}

/// エラーの中に埋め込まれた google.rpc.ErrorInfo の JSON から (モデル, 戻る時刻)
fn error_info(text: &str) -> Option<(String, i64)> {
    let start = text.find(r#"{"@type":"type.googleapis.com/google.rpc.ErrorInfo""#)?;
    let v: Value = serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Value>()
        .next()?
        .ok()?;
    if v["reason"] != "QUOTA_EXHAUSTED" {
        return None;
    }
    let m = &v["metadata"];
    Some((
        m["model"].as_str()?.to_string(),
        crate::scan::parse_ts(m["quotaResetTimeStamp"].as_str()?)?,
    ))
}

/// Muse のセッションの記録（直近 3 日分）から、枠を使い切って止まった記録（"usage window resets at …"）を拾う。
/// 戻る時刻がまだ来ていない一番新しいもの
pub fn muse_exhausted(root: &Path, now_ms: i64) -> Option<Agent> {
    let mut best: Option<(i64, i64)> = None; // (当たった時刻, 戻る時刻)
    // フォルダはこのマシンの日付で切られている
    let today = chrono::DateTime::from_timestamp_millis(now_ms)?
        .with_timezone(&chrono::Local)
        .date_naive();
    for days_ago in 0..3 {
        let day = today.checked_sub_days(chrono::Days::new(days_ago))?;
        let dir = root.join(day.format("%Y/%m/%d").to_string());
        for file in session_logs(&dir) {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            for line in text
                .lines()
                .filter(|l| l.contains("usage window resets at"))
            {
                let Some(hit) = muse_failure(line) else {
                    continue;
                };
                // もう戻った記録で、まだ戻っていない記録を隠さない
                if hit.1 > now_ms && best.is_none_or(|b| hit.0 > b.0) {
                    best = Some(hit);
                }
            }
        }
    }
    let (at, reset) = best?;
    Some(Agent {
        windows: vec![Window {
            kind: "window".into(),
            label: String::new(),
            used_percent: Some(100.0),
            resets_at: Some(reset),
            exhausted: true,
        }],
        observed_at: Some(at),
        plan: None,
    })
}

/// その日のフォルダの下の session.jsonl（サブエージェントの分も）
fn session_logs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "session.jsonl") {
                out.push(p);
            }
        }
    }
    out
}

/// 失敗の記録 1 行から (当たった時刻 ms, 戻る時刻 ms)。人の入力に同じ文が入っていても拾わないよう、失敗の記録だけ見る
fn muse_failure(line: &str) -> Option<(i64, i64)> {
    let v: Value = serde_json::from_str(line).ok()?;
    let ev = &v["payload"]["event"];
    if ev["kind"] != "failed" {
        return None;
    }
    let reason = ev["reason"].as_str()?;
    let after = reason.split("usage window resets at ").nth(1)?;
    let iso: String = after
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '(')
        .collect();
    let reset = crate::scan::parse_ts(iso.trim_end_matches('.'))?;
    // recorded_at はマイクロ秒
    Some((v["recorded_at"].as_i64()? / 1000, reset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_codex_rate_limits() {
        let line = |ts: &str, used: f64| {
            json!({"timestamp": ts, "type": "event_msg", "payload": {"type": "token_count", "rate_limits": {
                "limit_id": "codex", "primary": {"used_percent": used, "window_minutes": 10080, "resets_at": 1_791_595_339},
                "secondary": {"used_percent": 100.0, "window_minutes": 300, "resets_at": 1_791_000_000},
                "plan_type": "prolite"}}})
            .to_string()
        };
        let text = [
            line("2026-10-04T04:00:00Z", 5.0),
            line("2026-10-04T04:32:38Z", 9.0),
            r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":null}}"#.into(),
        ]
        .join("\n");
        let a = codex_from(&text).unwrap();
        assert_eq!(a.plan.as_deref(), Some("prolite"));
        assert_eq!(a.observed_at, crate::scan::parse_ts("2026-10-04T04:32:38Z"));
        assert_eq!(
            a.windows[0],
            Window {
                kind: "seven_day".into(),
                label: String::new(),
                used_percent: Some(9.0),
                resets_at: Some(1_791_595_339_000),
                exhausted: false,
            }
        );
        assert_eq!(
            (a.windows[1].kind.as_str(), a.windows[1].exhausted),
            ("five_hour", true)
        );
        assert!(codex_from("{}").is_none());
        assert_eq!(kind_of(60), "window");
        // ファイルから: 新しい記録に無ければ前の記録を見る
        let root = crate::db::temp_dir("limits-codex");
        let day = root.join("2026/10/04");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-a.jsonl"), &text).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(day.join("rollout-b.jsonl"), "{}\n").unwrap();
        assert_eq!(codex_limits(&root).unwrap().windows.len(), 2);
        assert!(codex_limits(&root.join("none")).is_none());
    }

    #[test]
    fn keeps_what_the_status_line_received() {
        let dir = crate::db::temp_dir("limits-statusline");
        let claude = r#"{"model":{"id":"x"},"rate_limits":{"five_hour":{"used_percentage":23.4,"resets_at":1792000000},"seven_day":{"used_percentage":41,"resets_at":1792500000}}}"#;
        assert_eq!(
            save_statusline(&dir, "claude", claude, 5).unwrap(),
            "5h 23% · 7d 41%"
        );
        let a = read_statusline(&dir, "claude").unwrap();
        assert_eq!(a.observed_at, Some(5));
        assert_eq!(a.windows[0].resets_at, Some(1_792_000_000_000));
        // 枠の無い回は前の値を残す
        save_statusline(&dir, "claude", r#"{"model":{}}"#, 9).unwrap();
        assert_eq!(
            read_statusline(&dir, "claude").unwrap().observed_at,
            Some(5)
        );
        assert!(save_statusline(&dir, "claude", "not json", 9).is_err());
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("claude.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // agy: 残りの割合と ISO の時刻
        let gemini = r#"{"plan_tier":"pro","quota":{"gemini-weekly":{"remaining_fraction":0.25,"reset_time":"2026-10-24T11:57:37Z"}}}"#;
        assert_eq!(
            save_statusline(&dir, "gemini", gemini, 7).unwrap(),
            "gemini-weekly 75%"
        );
        let g = read_statusline(&dir, "gemini").unwrap();
        assert_eq!(
            (g.plan.as_deref(), g.windows[0].used_percent),
            (Some("pro"), Some(75.0))
        );
        assert!(read_statusline(&dir, "muse").is_none());
    }

    #[test]
    fn finds_quota_errors_in_agy_conversations() {
        let dir = crate::db::temp_dir("limits-agy");
        let conn = rusqlite::Connection::open(dir.join("c1.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER, step_payload BLOB)",
        )
        .unwrap();
        let payload = "\u{1}\u{2}RESOURCE_EXHAUSTED (code 429): Individual quota reached. {\"@type\":\"type.googleapis.com/google.rpc.ErrorInfo\",\"reason\":\"QUOTA_EXHAUSTED\",\"domain\":\"cloudcode-pa.googleapis.com\",\"metadata\":{\"model\":\"gemini-4-argon-high\",\"quotaResetTimeStamp\":\"2026-10-24T11:57:37Z\"}}\u{3}trailing";
        conn.execute(
            "INSERT INTO steps VALUES (1, 17, ?1), (2, 3, ?1)",
            [payload.as_bytes()],
        )
        .unwrap();
        drop(conn);
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let a = gemini_exhausted(&dir).unwrap();
        assert_eq!(a.windows.len(), 1);
        assert_eq!(
            (a.windows[0].label.as_str(), a.windows[0].resets_at),
            (
                "gemini-4-argon-high",
                crate::scan::parse_ts("2026-10-24T11:57:37Z")
            )
        );
        assert!(error_info("no json here").is_none());
        assert!(
            error_info(r#"{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"OTHER"}"#)
                .is_none()
        );
        assert!(gemini_exhausted(&dir.join("none")).is_none());
        // 壊れた DB は飛ばす
        std::fs::write(dir.join("broken.db"), "x").unwrap();
        assert!(gemini_exhausted(&dir).is_some());
        // 別の会話に、同じモデルでもっと遅く戻る記録があればそちら（どの DB を先に見ても同じ）
        let later = rusqlite::Connection::open(dir.join("c2.db")).unwrap();
        later
            .execute_batch("CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER, step_payload BLOB)")
            .unwrap();
        later
            .execute(
                "INSERT INTO steps VALUES (1, 17, ?1)",
                [payload
                    .replace("2026-10-24T11:57:37Z", "2026-10-30T00:00:00Z")
                    .as_bytes()],
            )
            .unwrap();
        drop(later);
        let a = gemini_exhausted(&dir).unwrap();
        assert_eq!(
            a.windows[0].resets_at,
            crate::scan::parse_ts("2026-10-30T00:00:00Z")
        );
    }

    #[test]
    fn finds_muse_quota_failures() {
        let root = crate::db::temp_dir("limits-muse");
        let now = crate::scan::parse_ts("2026-10-21T00:00:00Z").unwrap();
        let day = chrono::DateTime::from_timestamp_millis(now)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y/%m/%d")
            .to_string();
        let dir = root.join(day).join("sess/subagent/x");
        std::fs::create_dir_all(&dir).unwrap();
        let failed = json!({"recorded_at": 1_792_530_933_202_760_i64, "payload": {"event": {"kind": "failed",
            "reason": "model failed: API error 429: Subscription quota exhausted. Your usage window resets at 2026-10-21T01:33:49Z. (rate_limit_error)"}}});
        // あとで当たったが、もう戻っている記録（まだ戻っていない上の記録を隠さない）
        let reset = json!({"recorded_at": 1_792_531_000_000_000_i64, "payload": {"event": {"kind": "failed",
            "reason": "Subscription quota exhausted. Your usage window resets at 2026-10-20T00:00:00Z. (rate_limit_error)"}}});
        let started = json!({"recorded_at": 1_792_530_999_000_000_i64, "payload": {"event": {"kind": "started",
            "prompt": "usage window resets at 2030-01-01T00:00:00Z"}}});
        std::fs::write(
            dir.join("session.jsonl"),
            format!("{failed}\n{started}\n{reset}\nnot json usage window resets at\n"),
        )
        .unwrap();
        let a = muse_exhausted(&root, now).unwrap();
        assert_eq!(a.observed_at, Some(1_792_530_933_202));
        assert_eq!(
            a.windows[0].resets_at,
            crate::scan::parse_ts("2026-10-21T01:33:49Z")
        );
        assert!(muse_exhausted(&root.join("none"), now).is_none());
    }

    #[test]
    fn hides_windows_that_have_reset() {
        let home = crate::db::temp_dir("limits-all");
        let day = home.join(".codex/sessions/2026/10/04");
        std::fs::create_dir_all(&day).unwrap();
        let line = json!({"timestamp": "2026-10-04T04:32:38Z", "payload": {"rate_limits": {
            "primary": {"used_percent": 9.0, "window_minutes": 10080, "resets_at": 2_000},
            "secondary": {"used_percent": 1.0, "window_minutes": 300, "resets_at": 1_000}}}});
        std::fs::write(day.join("rollout-a.jsonl"), line.to_string()).unwrap();
        let v = all(&home, &home.join("limits"), 1_500_000);
        let codex = v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["agent"] == "codex")
            .unwrap();
        // 1_000 秒に戻る 5 時間の枠は過ぎたので出さない
        assert_eq!(codex["windows"].as_array().unwrap().len(), 1);
        assert_eq!(codex["reset"], 1);
        assert_eq!(codex["setup"], Value::Null);
        let muse = v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["agent"] == "muse")
            .unwrap();
        assert_eq!(muse["windows"], json!([]));
        assert_eq!(merge(None, Some(Agent::default())), Some(Agent::default()));
        let at = |t| Agent {
            observed_at: Some(t),
            ..Default::default()
        };
        assert_eq!(
            merge(Some(at(1)), Some(at(5))).unwrap().observed_at,
            Some(5)
        );
    }
}
