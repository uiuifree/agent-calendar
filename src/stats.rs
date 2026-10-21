use crate::pricing::{self, Tokens};
use crate::repo;
use crate::scan::BUCKET_SECS;
use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// これより長く手が止まったら作業が切れたと見る。画面（layout.js の GAP_MS）と同じ値にしておく
pub const GAP_SECS: i64 = 30 * 60;
/// 案件を割り当てていないリポジトリ。画面で「未分類」「Unassigned」と表示する
pub const UNASSIGNED: &str = "";

/// 10 分の枠の集まりから作業時間（秒）を出す。間が GAP_SECS 以内なら続けて作業していたと見て、
/// 間の時間も数える。並行して開いていたセッションは枠を合わせてから数えるので二重にならない
pub fn active_secs(buckets: &BTreeSet<i64>) -> i64 {
    let mut total = 0;
    let mut seg: Option<(i64, i64)> = None;
    for &b in buckets {
        seg = match seg {
            Some((start, end)) if b - end <= GAP_SECS => Some((start, b + BUCKET_SECS)),
            Some((start, end)) => {
                total += end - start;
                Some((b, b + BUCKET_SECS))
            }
            None => Some((b, b + BUCKET_SECS)),
        };
    }
    total + seg.map_or(0, |(s, e)| e - s)
}

/// 費用の合計。単価の分からないモデルは足さずに名前を返す
#[derive(Debug, Default, PartialEq)]
pub struct Cost {
    pub usd: f64,
    pub tokens: Tokens,
    pub unpriced: BTreeSet<String>,
}

impl Cost {
    pub fn add(&mut self, model: &str, fast: bool, t: &Tokens) {
        self.tokens.add(t);
        match pricing::cost(model, fast, t) {
            Some(c) => self.usd += c,
            None => {
                self.unpriced.insert(if fast {
                    format!("{model}（fast）")
                } else {
                    model.to_string()
                });
            }
        }
    }

    pub fn to_json(&self) -> Value {
        let t = &self.tokens;
        json!({
            "usd": self.usd,
            "tokens": { "input": t.input, "output": t.output, "cache_read": t.cache_read, "cache_5m": t.cache_5m, "cache_1h": t.cache_1h },
            "unpriced": self.unpriced,
        })
    }
}

fn read_tokens(r: &rusqlite::Row, from: usize) -> rusqlite::Result<Tokens> {
    Ok(Tokens {
        input: r.get(from)?,
        output: r.get(from + 1)?,
        cache_read: r.get(from + 2)?,
        cache_5m: r.get(from + 3)?,
        cache_1h: r.get(from + 4)?,
    })
}

/// セッションの費用を本体・advisor・サブエージェントに分けて出す
pub fn session_cost(conn: &Connection, id: &str) -> Result<Value> {
    let mut stmt = conn.prepare(
        "SELECT kind, model, fast, SUM(input), SUM(output), SUM(cache_read), SUM(cache_5m), SUM(cache_1h)
         FROM usage WHERE session_id = ?1 GROUP BY kind, model, fast",
    )?;
    let mut by_kind: BTreeMap<String, Cost> = BTreeMap::new();
    let mut total = Cost::default();
    let mut models = BTreeSet::new();
    let mut rows = stmt.query([id])?;
    while let Some(r) = rows.next()? {
        let (kind, model, fast): (String, String, bool) = (r.get(0)?, r.get(1)?, r.get(2)?);
        let t = read_tokens(r, 3)?;
        by_kind.entry(kind).or_default().add(&model, fast, &t);
        total.add(&model, fast, &t);
        models.insert(model);
    }
    let kinds: serde_json::Map<String, Value> = by_kind
        .iter()
        .map(|(k, c)| (k.clone(), c.to_json()))
        .collect();
    Ok(json!({ "total": total.to_json(), "by_kind": kinds, "models": models }))
}

/// そのセッションの、from〜to（unix 秒）の 10 分の枠に入った分の費用（予定の 1 回ごとの費用。
/// 前回に続ける予定は同じセッションに何回も書くので、実行していた時間で切る）
pub fn cost_between(conn: &Connection, id: &str, from: i64, to: i64) -> Result<Cost> {
    let mut stmt = conn.prepare(
        "SELECT model, fast, SUM(input), SUM(output), SUM(cache_read), SUM(cache_5m), SUM(cache_1h)
         FROM usage WHERE session_id = ?1 AND bucket >= ?2 AND bucket <= ?3 GROUP BY model, fast",
    )?;
    let mut cost = Cost::default();
    let mut rows = stmt.query(rusqlite::params![id, from / BUCKET_SECS * BUCKET_SECS, to])?;
    while let Some(r) = rows.next()? {
        let (model, fast): (String, bool) = (r.get(0)?, r.get(1)?);
        cost.add(&model, fast, &read_tokens(r, 2)?);
    }
    Ok(cost)
}

/// 予定のすべての回の費用の合計。前回に続ける予定は同じセッションに書くので、回ごとの費用を足すと
/// 境目の 10 分の枠を二重に数える。どれかの回の時間に入る枠を 1 回ずつ数える
pub fn schedule_cost(conn: &Connection, schedule_id: i64) -> Result<Cost> {
    let mut stmt = conn.prepare(
        "SELECT u.model, u.fast, SUM(u.input), SUM(u.output), SUM(u.cache_read), SUM(u.cache_5m), SUM(u.cache_1h)
         FROM usage u
         WHERE EXISTS (SELECT 1 FROM schedule_runs r
                       WHERE r.schedule_id = ?1 AND r.session_id = u.session_id AND r.finished_at IS NOT NULL
                         AND u.bucket >= r.started_at / 1000 / ?2 * ?2 AND u.bucket <= r.finished_at / 1000)
         GROUP BY u.model, u.fast",
    )?;
    let mut cost = Cost::default();
    let mut rows = stmt.query(rusqlite::params![schedule_id, BUCKET_SECS])?;
    while let Some(r) = rows.next()? {
        let (model, fast): (String, bool) = (r.get(0)?, r.get(1)?);
        cost.add(&model, fast, &read_tokens(r, 2)?);
    }
    Ok(cost)
}

#[derive(Default)]
struct RepoAgg {
    buckets: BTreeSet<i64>,
    sessions: HashSet<String>,
    cost: Cost,
}

/// 期間（unix 秒）の作業時間と費用を、案件 → リポジトリの順に束ねて出す。
/// machines を渡すとそのホストのセッションだけ（手元は空文字）。None なら全部。
/// exclude のリポジトリは数えない（サイドバーでチェックを外したもの）。
/// モデル別・種類別（本体・サブエージェント・advisor）の内訳も付ける
pub fn stats(
    conn: &Connection,
    from: i64,
    to: i64,
    machines: Option<&[String]>,
    exclude: &[String],
) -> Result<Value> {
    // SQLite の json_each で IN を組む（ホストの数が変わってもプレースホルダを組み立て直さずに済む）
    let machine: Option<String> = machines.map(serde_json::to_string).transpose()?;
    let exclude = serde_json::to_string(exclude)?;
    let mut repos: HashMap<String, RepoAgg> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT s.repo, a.session_id, a.bucket FROM activity a JOIN sessions s ON s.id = a.session_id
         WHERE a.bucket >= ?1 AND a.bucket < ?2 AND (?3 IS NULL OR s.machine IN (SELECT value FROM json_each(?3)))
           AND s.repo NOT IN (SELECT value FROM json_each(?4))",
    )?;
    let mut rows = stmt.query(rusqlite::params![from, to, machine, exclude])?;
    while let Some(r) = rows.next()? {
        let agg = repos.entry(r.get(0)?).or_default();
        agg.sessions.insert(r.get(1)?);
        agg.buckets.insert(r.get(2)?);
    }
    let mut stmt = conn.prepare(
        "SELECT s.repo, u.model, u.fast, SUM(u.input), SUM(u.output), SUM(u.cache_read), SUM(u.cache_5m), SUM(u.cache_1h),
                u.kind
         FROM usage u JOIN sessions s ON s.id = u.session_id
         WHERE u.bucket >= ?1 AND u.bucket < ?2 AND (?3 IS NULL OR s.machine IN (SELECT value FROM json_each(?3)))
           AND s.repo NOT IN (SELECT value FROM json_each(?4))
         GROUP BY s.repo, u.model, u.fast, u.kind",
    )?;
    let mut rows = stmt.query(rusqlite::params![from, to, machine, exclude])?;
    let (mut by_model, mut by_kind) = (
        BTreeMap::<String, Cost>::new(),
        BTreeMap::<String, Cost>::new(),
    );
    let mut total = Cost::default();
    while let Some(r) = rows.next()? {
        let (model, fast, kind): (String, bool, String) = (r.get(1)?, r.get(2)?, r.get(8)?);
        let t = read_tokens(r, 3)?;
        repos
            .entry(r.get(0)?)
            .or_default()
            .cost
            .add(&model, fast, &t);
        by_model
            .entry(model.clone())
            .or_default()
            .add(&model, fast, &t);
        by_kind.entry(kind).or_default().add(&model, fast, &t);
        total.add(&model, fast, &t);
    }
    let mut models: Vec<Value> = by_model
        .iter()
        .map(|(m, c)| {
            let mut v = c.to_json();
            v["model"] = m.clone().into();
            v["unpriced"] = (!c.unpriced.is_empty()).into();
            v
        })
        .collect();
    models.sort_by(|a, b| {
        let key = |v: &Value| (v["usd"].as_f64().unwrap_or(0.0), tokens_of(v));
        key(b)
            .partial_cmp(&key(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let kinds: serde_json::Map<String, Value> = by_kind
        .iter()
        .map(|(k, c)| (k.clone(), c.to_json()))
        .collect();

    let assigned = projects(conn)?;
    let mut groups: BTreeMap<String, Vec<(&String, &RepoAgg)>> = BTreeMap::new();
    for (name, agg) in &repos {
        let project = assigned
            .get(name)
            .cloned()
            .unwrap_or_else(|| UNASSIGNED.to_string());
        groups.entry(project).or_default().push((name, agg));
    }
    let mut all = BTreeSet::new();
    let mut total_cost = 0.0;
    let mut out = Vec::new();
    for (project, mut list) in groups {
        list.sort_by_key(|(_, a)| std::cmp::Reverse(active_secs(&a.buckets)));
        let mut union = BTreeSet::new();
        let mut usd = 0.0;
        let mut sessions = 0;
        let mut unpriced = false;
        let rows: Vec<Value> = list
            .iter()
            .map(|(name, a)| {
                union.extend(&a.buckets);
                usd += a.cost.usd;
                sessions += a.sessions.len();
                unpriced |= !a.cost.unpriced.is_empty();
                json!({
                    "repo": name, "name": repo::name(name), "secs": active_secs(&a.buckets),
                    "sessions": a.sessions.len(), "usd": a.cost.usd, "unpriced": a.cost.unpriced,
                })
            })
            .collect();
        all.extend(&union);
        total_cost += usd;
        out.push(json!({ "project": project, "secs": active_secs(&union), "usd": usd, "sessions": sessions, "unpriced": unpriced, "repos": rows }));
    }
    out.sort_by_key(|p| std::cmp::Reverse(p["secs"].as_i64().unwrap_or(0)));
    let known: BTreeSet<&String> = assigned.values().collect();
    let tokens = total.to_json()["tokens"].clone();
    Ok(json!({
        "gap_secs": GAP_SECS,
        "total": {
            "secs": active_secs(&all), "usd": total_cost, "unpriced": !total.unpriced.is_empty(), "tokens": tokens,
        },
        "projects": out,
        "known_projects": known,
        "by_model": models,
        "by_kind": kinds,
    }))
}

fn tokens_of(v: &Value) -> f64 {
    let t = &v["tokens"];
    ["input", "output", "cache_read", "cache_5m", "cache_1h"]
        .iter()
        .map(|k| t[k].as_f64().unwrap_or(0.0))
        .sum()
}

/// 集計を表にする（CLI の stats）。csv なら CSV（表計算や請求の明細へ）、でなければタブ区切り。
/// 1 行 = 1 リポジトリ。時間は時間単位（小数 1 桁）、金額は API 単価の換算（単価の分からないモデルを含めば +）
pub fn table(v: &Value, csv: bool) -> String {
    let field = |s: &str| {
        if csv && (s.contains(',') || s.contains('"') || s.contains('\n')) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let sep = if csv { "," } else { "\t" };
    let mut out = ["project", "repository", "hours", "sessions", "usd"].join(sep) + "\n";
    for p in v["projects"].as_array().into_iter().flatten() {
        for r in p["repos"].as_array().into_iter().flatten() {
            let unpriced = r["unpriced"].as_array().is_some_and(|u| !u.is_empty());
            let row = [
                field(p["project"].as_str().unwrap_or("")),
                field(r["repo"].as_str().unwrap_or("")),
                format!("{:.1}", r["secs"].as_f64().unwrap_or(0.0) / 3600.0),
                r["sessions"].to_string(),
                format!(
                    "{:.2}{}",
                    r["usd"].as_f64().unwrap_or(0.0),
                    if unpriced { "+" } else { "" }
                ),
            ];
            out.push_str(&row.join(sep));
            out.push('\n');
        }
    }
    out
}

/// 期間のレポート（CLI の report）。集計・残タスク・保存した期間の要約を Markdown で
pub fn report(
    v: &Value,
    todo: &[crate::finished::Todo],
    unsummarized: usize,
    summary: Option<&str>,
    (from, to): (i64, i64),
) -> String {
    let day = |secs: i64| {
        chrono::DateTime::from_timestamp(secs, 0)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .unwrap_or_default()
    };
    let hours = |x: &Value| x["secs"].as_f64().unwrap_or(0.0) / 3600.0;
    let usd = |x: &Value| {
        format!(
            "${:.2}{}",
            x["usd"].as_f64().unwrap_or(0.0),
            if x["unpriced"] == true { "+" } else { "" }
        )
    };
    let total = &v["total"];
    let mut out = format!(
        "# {} – {}\n\n{:.1} h · {}\n",
        day(from),
        day(to - 1),
        hours(total),
        usd(total)
    );
    if let Some(text) = summary {
        out.push_str(&format!("\n## Summary\n\n{}\n", text.trim()));
    }
    out.push_str("\n## Projects\n\n");
    for p in v["projects"].as_array().into_iter().flatten() {
        let name = p["project"]
            .as_str()
            .filter(|n| !n.is_empty())
            .unwrap_or("(unassigned)");
        out.push_str(&format!("- {name}: {:.1} h · {}\n", hours(p), usd(p)));
        for r in p["repos"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "  - {}: {:.1} h · {} sessions\n",
                r["name"].as_str().unwrap_or(""),
                hours(r),
                r["sessions"]
            ));
        }
    }
    out.push_str("\n## Left to do\n\n");
    if todo.is_empty() {
        out.push_str("- (nothing)\n");
    }
    for t in todo {
        let repo = t.repo.rsplit('/').next().unwrap_or(&t.repo);
        let next = if t.next.is_empty() {
            String::new()
        } else {
            format!(" — {}", t.next)
        };
        out.push_str(&format!("- [{repo}] {} ({}){next}\n", t.title, t.status));
    }
    if unsummarized > 0 {
        out.push_str(&format!(
            "\n{unsummarized} sessions are not summarized yet, so they are not in this list.\n"
        ));
    }
    out.push_str("\nCosts are estimates at API prices, not your subscription bill.\n");
    out
}

pub fn projects(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT repo, project FROM repo_projects")?;
    let map = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<HashMap<String, String>>>()?;
    Ok(map)
}

/// 空文字は割り当ての解除
pub fn set_project(conn: &Connection, repo: &str, project: &str) -> Result<()> {
    let project = project.trim();
    if project == UNASSIGNED {
        conn.execute("DELETE FROM repo_projects WHERE repo = ?1", [repo])?;
    } else {
        conn.execute(
            "INSERT INTO repo_projects (repo, project) VALUES (?1, ?2)
             ON CONFLICT(repo) DO UPDATE SET project = excluded.project",
            [repo, project],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use rusqlite::params;

    fn set(v: &[i64]) -> BTreeSet<i64> {
        v.iter().map(|m| m * 60).collect()
    }

    #[test]
    fn gap_matches_the_calendar() {
        let js = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/web/src/layout.js"))
            .unwrap();
        assert!(
            js.contains(&format!("GAP_MS = {} * 60 * 1000", GAP_SECS / 60)),
            "layout.js の GAP_MS と GAP_SECS がずれている"
        );
    }

    #[test]
    fn active_time_merges_short_gaps() {
        assert_eq!(active_secs(&BTreeSet::new()), 0);
        assert_eq!(active_secs(&set(&[0])), 600);
        // 0:00 の枠（〜0:10）と 0:40 の枠は 30 分差なので続ける → 0:00〜0:50
        assert_eq!(active_secs(&set(&[0, 40])), 50 * 60);
        // 0:00 と 0:50 は 40 分空くので分ける → 10 分 + 10 分
        assert_eq!(active_secs(&set(&[0, 50])), 20 * 60);
        assert_eq!(
            active_secs(&set(&[0, 10, 60, 70, 200])),
            (20 + 20 + 10) * 60
        );
    }

    fn seed(conn: &Connection, id: &str, repo: &str, buckets: &[i64], model: &str, output: i64) {
        conn.execute(
            "INSERT INTO sessions VALUES (?1, ?1, 0, 0, ?2, ?2, 'main', '', 0, 0, 'u', 1, 'claude', '', 0)",
            params![id, repo],
        )
        .unwrap();
        for b in buckets {
            conn.execute("INSERT INTO activity VALUES (?1, ?2, 1)", params![id, b])
                .unwrap();
        }
        conn.execute(
            "INSERT INTO usage VALUES (?1, ?2, ?3, 'main', 0, 0, ?4, 0, 0, 0)",
            params![id, buckets[0], model, output],
        )
        .unwrap();
    }

    #[test]
    fn stats_group_by_project_without_double_counting() {
        let conn = db::open_at(&db::temp_dir("stats").join("t.db")).unwrap();
        // a と b は同じ時間帯に並行して開いていた
        seed(
            &conn,
            "a",
            "/r/acme-api",
            &[6000, 6600],
            "claude-opus-5-5",
            1_000_000,
        );
        seed(
            &conn,
            "b",
            "/r/acme-web",
            &[6000],
            "claude-opus-5-5",
            1_000_000,
        );
        seed(&conn, "c", "/r/private", &[10200], "mystery-model", 10);
        set_project(&conn, "/r/acme-api", "acme").unwrap();
        set_project(&conn, "/r/acme-web", " acme ").unwrap();

        let s = stats(&conn, 0, 100_000, None, &[]).unwrap();
        let p = &s["projects"];
        assert_eq!(p[0]["project"], "acme");
        assert_eq!(p[0]["secs"], 1200); // 2 リポジトリ合わせても 6000〜7200 の 20 分
        assert_eq!(p[0]["sessions"], 2);
        assert_eq!(p[0]["usd"], 40.0);
        assert_eq!(p[0]["repos"][0]["name"], "acme-api");
        assert_eq!(p[1]["project"], UNASSIGNED);
        assert_eq!(p[1]["repos"][0]["unpriced"], json!(["mystery-model"]));
        assert_eq!(s["total"]["secs"], 1800);
        assert_eq!(s["known_projects"], json!(["acme"]));
        // 単価の分からないモデルを含む案件・全体には印が付く
        assert_eq!(
            (p[0]["unpriced"].as_bool(), p[1]["unpriced"].as_bool()),
            (Some(false), Some(true))
        );
        assert_eq!(s["total"]["unpriced"], true);
        // モデル別（金額の大きい順）・種類別の内訳
        assert_eq!(s["by_model"][0]["model"], "claude-opus-5-5");
        assert_eq!(s["by_model"][0]["usd"], 40.0);
        assert_eq!(s["by_model"][1]["model"], "mystery-model");
        assert_eq!(s["by_model"][1]["unpriced"], true);
        assert_eq!(s["by_kind"]["main"]["tokens"]["output"], 2_000_010);
        assert_eq!(s["total"]["tokens"]["output"], 2_000_010);
        // 表とレポート（CLI）
        let csv = table(&s, true);
        assert!(csv.starts_with("project,repository,hours,sessions,usd\n"));
        assert!(csv.contains("acme,/r/acme-api,0.3,1,20.00\n"), "{csv}");
        assert!(csv.contains(",/r/private,0.2,1,0.00+\n"), "{csv}");
        assert!(table(&s, false).contains("acme\t/r/acme-api\t0.3\t1\t20.00\n"));
        let todo = [crate::finished::Todo {
            id: "a".into(),
            repo: "/r/acme-api".into(),
            machine: String::new(),
            title: "題".into(),
            status: "wip".into(),
            next: "push".into(),
            last_ts: 0,
        }];
        let md = report(&s, &todo, 2, Some("よくやった"), (0, 86400));
        assert!(md.contains("## Summary\n\nよくやった"), "{md}");
        assert!(md.contains("- acme: 0.3 h · $40.00\n"), "{md}");
        assert!(md.contains("- (unassigned): 0.2 h · $0.00+\n"), "{md}");
        assert!(md.contains("- [acme-api] 題 (wip) — push\n"), "{md}");
        assert!(md.contains("2 sessions are not summarized yet"));
        assert!(report(&s, &[], 0, None, (0, 86400)).contains("- (nothing)"));
        // サイドバーで外したリポジトリは数えない
        let without = stats(&conn, 0, 100_000, None, &["/r/private".to_string()]).unwrap();
        assert_eq!(without["total"]["secs"], 1200);
        assert_eq!(without["total"]["unpriced"], false);
        assert_eq!(s["gap_secs"], GAP_SECS);

        // 解除すると未分類に戻る
        set_project(&conn, "/r/acme-web", "").unwrap();
        set_project(&conn, "/r/acme-api", " ").unwrap();
        assert!(projects(&conn).unwrap().is_empty());
        // 範囲外は数えない
        assert_eq!(
            stats(&conn, 7000, 8000, None, &[]).unwrap()["total"]["secs"],
            0
        );
        // ホストで絞る
        conn.execute("UPDATE sessions SET machine = 'ai-node' WHERE id = 'c'", [])
            .unwrap();
        let node = stats(&conn, 0, 100_000, Some(&["ai-node".to_string()]), &[]).unwrap();
        assert_eq!(node["total"]["secs"], 600);
        assert_eq!(node["projects"][0]["repos"][0]["name"], "private");
        let mine = [String::new()];
        assert_eq!(
            stats(&conn, 0, 100_000, Some(&mine), &[]).unwrap()["total"]["secs"],
            1200
        );
        let both = [String::new(), "ai-node".to_string()];
        assert_eq!(
            stats(&conn, 0, 100_000, Some(&both), &[]).unwrap()["total"]["secs"],
            1800
        );
        assert_eq!(
            stats(&conn, 0, 100_000, Some(&[]), &[]).unwrap()["total"]["secs"],
            0
        );
    }

    #[test]
    fn session_cost_by_kind() {
        let conn = db::open_at(&db::temp_dir("cost").join("t.db")).unwrap();
        seed(&conn, "a", "/r/x", &[6000], "claude-opus-5-5", 1_000_000);
        conn.execute("INSERT INTO usage VALUES ('a', 6000, 'claude-fable-5-1', 'advisor', 0, 1000000, 0, 0, 0, 0)", []).unwrap();
        conn.execute(
            "INSERT INTO usage VALUES ('a', 6000, 'claude-opus-5-5', 'subagent', 1, 0, 1, 0, 0, 0)",
            [],
        )
        .unwrap();
        let c = session_cost(&conn, "a").unwrap();
        assert_eq!(c["by_kind"]["main"]["usd"], 20.0);
        assert_eq!(c["by_kind"]["advisor"]["usd"], 10.0);
        assert_eq!(c["total"]["usd"], 30.0);
        assert_eq!(c["total"]["unpriced"], json!(["claude-opus-5-5（fast）"]));
        assert_eq!(c["total"]["tokens"]["output"], 1_000_001);
        assert_eq!(c["models"], json!(["claude-fable-5-1", "claude-opus-5-5"]));
    }
}
