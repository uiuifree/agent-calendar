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

#[derive(Default)]
struct RepoAgg {
    buckets: BTreeSet<i64>,
    sessions: HashSet<String>,
    cost: Cost,
}

/// 期間（unix 秒）の作業時間と費用を、案件 → リポジトリの順に束ねて出す。
/// machines を渡すとそのホストのセッションだけ（手元は空文字）。None なら全部
pub fn stats(conn: &Connection, from: i64, to: i64, machines: Option<&[String]>) -> Result<Value> {
    // SQLite の json_each で IN を組む（ホストの数が変わってもプレースホルダを組み立て直さずに済む）
    let machine: Option<String> = machines.map(serde_json::to_string).transpose()?;
    let mut repos: HashMap<String, RepoAgg> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT s.repo, a.session_id, a.bucket FROM activity a JOIN sessions s ON s.id = a.session_id
         WHERE a.bucket >= ?1 AND a.bucket < ?2 AND (?3 IS NULL OR s.machine IN (SELECT value FROM json_each(?3)))",
    )?;
    let mut rows = stmt.query(rusqlite::params![from, to, machine])?;
    while let Some(r) = rows.next()? {
        let agg = repos.entry(r.get(0)?).or_default();
        agg.sessions.insert(r.get(1)?);
        agg.buckets.insert(r.get(2)?);
    }
    let mut stmt = conn.prepare(
        "SELECT s.repo, u.model, u.fast, SUM(u.input), SUM(u.output), SUM(u.cache_read), SUM(u.cache_5m), SUM(u.cache_1h)
         FROM usage u JOIN sessions s ON s.id = u.session_id
         WHERE u.bucket >= ?1 AND u.bucket < ?2 AND (?3 IS NULL OR s.machine IN (SELECT value FROM json_each(?3)))
         GROUP BY s.repo, u.model, u.fast",
    )?;
    let mut rows = stmt.query(rusqlite::params![from, to, machine])?;
    while let Some(r) = rows.next()? {
        let (model, fast): (String, bool) = (r.get(1)?, r.get(2)?);
        let t = read_tokens(r, 3)?;
        repos
            .entry(r.get(0)?)
            .or_default()
            .cost
            .add(&model, fast, &t);
    }

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
        let rows: Vec<Value> = list
            .iter()
            .map(|(name, a)| {
                union.extend(&a.buckets);
                usd += a.cost.usd;
                sessions += a.sessions.len();
                json!({
                    "repo": name, "name": repo::name(name), "secs": active_secs(&a.buckets),
                    "sessions": a.sessions.len(), "usd": a.cost.usd, "unpriced": a.cost.unpriced,
                })
            })
            .collect();
        all.extend(&union);
        total_cost += usd;
        out.push(json!({ "project": project, "secs": active_secs(&union), "usd": usd, "sessions": sessions, "repos": rows }));
    }
    out.sort_by_key(|p| std::cmp::Reverse(p["secs"].as_i64().unwrap_or(0)));
    let known: BTreeSet<&String> = assigned.values().collect();
    Ok(json!({
        "gap_secs": GAP_SECS,
        "total": { "secs": active_secs(&all), "usd": total_cost },
        "projects": out,
        "known_projects": known,
    }))
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
            "INSERT INTO sessions VALUES (?1, ?1, 0, 0, ?2, ?2, 'main', '', 0, 0, 'u', 1, 'claude', '')",
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

        let s = stats(&conn, 0, 100_000, None).unwrap();
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
        assert_eq!(s["gap_secs"], GAP_SECS);

        // 解除すると未分類に戻る
        set_project(&conn, "/r/acme-web", "").unwrap();
        set_project(&conn, "/r/acme-api", " ").unwrap();
        assert!(projects(&conn).unwrap().is_empty());
        // 範囲外は数えない
        assert_eq!(stats(&conn, 7000, 8000, None).unwrap()["total"]["secs"], 0);
        // ホストで絞る
        conn.execute("UPDATE sessions SET machine = 'ai-node' WHERE id = 'c'", [])
            .unwrap();
        let node = stats(&conn, 0, 100_000, Some(&["ai-node".to_string()])).unwrap();
        assert_eq!(node["total"]["secs"], 600);
        assert_eq!(node["projects"][0]["repos"][0]["name"], "private");
        let mine = [String::new()];
        assert_eq!(
            stats(&conn, 0, 100_000, Some(&mine)).unwrap()["total"]["secs"],
            1200
        );
        let both = [String::new(), "ai-node".to_string()];
        assert_eq!(
            stats(&conn, 0, 100_000, Some(&both)).unwrap()["total"]["secs"],
            1800
        );
        assert_eq!(
            stats(&conn, 0, 100_000, Some(&[])).unwrap()["total"]["secs"],
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
