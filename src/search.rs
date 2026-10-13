//! セッションの検索: リポジトリ・ブランチ・作業場所・題・要約・依頼の文章から、続きをしたい会話を探す。
//! 空白で区切った語はすべて含むもの（例: `agent-calendar feature/search`）。続きはこのマシンでしか出せないので、手元のものだけ
use crate::{finished, pins, summarize};
use anyhow::Result;
use rusqlite::{Connection, params_from_iter};
use serde_json::{Value, json};
use std::path::Path;

/// LIKE の中で `%` `_` を文字のまま探す
fn like(term: &str) -> String {
    let escaped = term
        .replace('\\', r"\\")
        .replace('%', r"\%")
        .replace('_', r"\_");
    format!("%{escaped}%")
}

/// 新しいものから limit 件。語が無ければ空
pub fn sessions(conn: &Connection, q: &str, limit: i64) -> Result<Vec<Value>> {
    let terms: Vec<String> = q.split_whitespace().map(like).collect();
    if terms.is_empty() {
        return Ok(vec![]);
    }
    // 作業場所はフォルダの名前だけ比べる（worktree の置き場所の親のパスで、ほかのリポジトリまで当たらないように）
    let cwd_name = "substr(s.cwd, length(rtrim(s.cwd, replace(s.cwd, '/', ''))) + 1)";
    let cond = (1..=terms.len())
        .map(|i| {
            format!(
                "(s.repo LIKE ?{i} ESCAPE '\\' OR s.branch LIKE ?{i} ESCAPE '\\' OR {cwd_name} LIKE ?{i} ESCAPE '\\'
                  OR s.title LIKE ?{i} ESCAPE '\\' OR m.body LIKE ?{i} ESCAPE '\\'
                  OR EXISTS (SELECT 1 FROM prompts p WHERE p.session_id = s.id AND p.text LIKE ?{i} ESCAPE '\\'))"
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let mut stmt = conn.prepare(&format!(
        "SELECT s.id, s.repo, s.branch, s.cwd, s.title, m.body,
                (SELECT text FROM prompts p WHERE p.session_id = s.id ORDER BY seq LIMIT 1),
                s.last_ts, s.source, {}
         FROM sessions s LEFT JOIN summaries m ON m.session_id = s.id
         WHERE s.machine = '' AND {cond}
         ORDER BY s.last_ts DESC LIMIT {limit}",
        finished::SQL
    ))?;
    let rows = stmt
        .query_map(params_from_iter(&terms), |r| {
            let summary = r
                .get::<_, Option<String>>(5)?
                .and_then(|b| summarize::read(&b));
            let cwd: String = r.get(3)?;
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "repo": r.get::<_, String>(1)?,
                "branch": r.get::<_, String>(2)?,
                // 作業場所が消えていると、続きはリポジトリの本体（別のブランチ）で動く
                "cwd_gone": !Path::new(&cwd).is_dir(),
                "cwd": cwd,
                "title": pins::title(summary.as_ref(), r.get(4)?, r.get(6)?),
                "status": finished::status(summary.as_ref(), r.get(9)?),
                "last_ts": r.get::<_, i64>(7)?,
                "source": r.get::<_, String>(8)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn session(conn: &Connection, id: &str, repo: &str, branch: &str, cwd: &str, last_ts: i64) {
        conn.execute(
            "INSERT INTO sessions VALUES (?1, ?1, 0, 0, ?2, ?3, ?4, '', 0, ?5, 'u', 1, 'claude', '')",
            params![id, repo, cwd, branch, last_ts],
        )
        .unwrap();
    }

    fn ids(v: &[Value]) -> Vec<&str> {
        v.iter().map(|s| s["id"].as_str().unwrap()).collect()
    }

    #[test]
    fn matches_every_term_newest_first() {
        let conn = crate::db::open_at(&crate::db::temp_dir("search").join("t.db")).unwrap();
        let here = "/"; // ある作業場所
        session(
            &conn,
            "a",
            "/src/agent-calendar",
            "feature/search",
            here,
            1000,
        );
        session(&conn, "b", "/src/agent-calendar", "main", "/gone", 3000);
        session(&conn, "c", "/src/other", "feature/search", here, 2000);
        session(
            &conn,
            "d",
            "/src/agent-calendar",
            "main",
            "/wt/feature-search",
            4000,
        );
        session(
            &conn,
            "e",
            "/src/agent-calendar",
            "feature/search",
            here,
            5000,
        );
        // 置き場所の親のパスに名前が入っていても、ほかのリポジトリは当たらない
        session(
            &conn,
            "f",
            "/src/other",
            "feature/x",
            "/share/agent-calendar/wt/x",
            6000,
        );
        conn.execute("UPDATE sessions SET machine = 'ai-node' WHERE id = 'e'", [])
            .unwrap();

        let got = sessions(&conn, "agent-calendar feature", 50).unwrap();
        // 別のマシンのものは出さない。作業場所のパスでも当たる（途中でブランチを切ったもの）
        assert_eq!(ids(&got), ["d", "a"]);
        assert_eq!(got[1]["branch"], "feature/search");
        assert_eq!(got[1]["cwd_gone"], false);
        assert_eq!(got[0]["cwd_gone"], true);
        assert_eq!(sessions(&conn, "search", 1).unwrap().len(), 1);
        assert!(sessions(&conn, "  ", 50).unwrap().is_empty());
    }

    #[test]
    fn matches_title_summary_and_prompts() {
        let conn = crate::db::open_at(&crate::db::temp_dir("search-text").join("t.db")).unwrap();
        session(&conn, "a", "/r", "main", "/r", 1000);
        session(&conn, "b", "/r", "main", "/r", 2000);
        session(&conn, "c", "/r", "main", "/r", 3000);
        conn.execute("UPDATE sessions SET title = 'Fix login' WHERE id = 'a'", [])
            .unwrap();
        conn.execute(
            "INSERT INTO summaries (session_id, last_uuid, model, body, created_at) VALUES ('b', 'u', 'm', ?1, 0)",
            [r#"{"title":"検索を足す","bullets":["一覧を作った"],"status":"wip"}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO prompts VALUES ('c', 0, 3000, '請求書の PDF を直して')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO prompts VALUES ('c', 1, 3001, 'テストも')", [])
            .unwrap();

        assert_eq!(ids(&sessions(&conn, "login", 50).unwrap()), ["a"]);
        let b = sessions(&conn, "一覧", 50).unwrap();
        assert_eq!(ids(&b), ["b"]);
        assert_eq!(b[0]["title"], "検索を足す");
        assert_eq!(b[0]["status"], "wip");
        // 語ごとに別の依頼に入っていてもよい
        let c = sessions(&conn, "PDF テスト", 50).unwrap();
        assert_eq!(ids(&c), ["c"]);
        assert_eq!(c[0]["title"], "請求書の PDF を直して");
    }

    #[test]
    fn wildcards_are_literal() {
        let conn = crate::db::open_at(&crate::db::temp_dir("search-like").join("t.db")).unwrap();
        session(&conn, "a", "/r", "fix_100%", "/r", 1000);
        session(&conn, "b", "/r", "fixa1000", "/r", 2000);
        session(&conn, "c", "/r", r"back\slash", "/r", 3000);
        assert_eq!(ids(&sessions(&conn, "fix_100%", 50).unwrap()), ["a"]);
        assert_eq!(ids(&sessions(&conn, "%", 50).unwrap()), ["a"]);
        assert_eq!(ids(&sessions(&conn, r"k\s", 50).unwrap()), ["c"]);
    }
}
