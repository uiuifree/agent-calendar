//! ピン留め: 進行中の会話をすぐ開けるように、選んだセッションをサイドバーの上に並べる。
//! あわせて、リポジトリの画面に出す「そのリポジトリのセッション」の一覧も作る（題の付け方が同じなので）
use crate::summarize;
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

/// 画面に出す題: 要約の題 → Claude Code が付けた題 → 最初の依頼の書き出し
pub fn title(
    summary: Option<&summarize::Summary>,
    ai_title: String,
    first_prompt: Option<String>,
) -> String {
    summary
        .map(|s| s.title.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| Some(ai_title).filter(|t| !t.is_empty()))
        .or(first_prompt.map(|p| p.chars().take(40).collect()))
        .unwrap_or_default()
}

/// 付ける・外す（何度押しても同じ）
pub fn set(conn: &Connection, id: &str, pinned: bool, now_ms: i64) -> Result<()> {
    if pinned {
        conn.execute(
            "INSERT OR IGNORE INTO pins (session_id, pinned_at) VALUES (?1, ?2)",
            params![id, now_ms],
        )?;
    } else {
        conn.execute("DELETE FROM pins WHERE session_id = ?1", [id])?;
    }
    Ok(())
}

pub fn is_pinned(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .prepare("SELECT 1 FROM pins WHERE session_id = ?1")?
        .exists([id])?)
}

/// ピン留めしたセッション。最近動いたものから
pub fn list(conn: &Connection) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.repo, s.title, m.body,
                (SELECT text FROM prompts p WHERE p.session_id = s.id ORDER BY seq LIMIT 1),
                s.last_ts, s.source, s.machine
         FROM pins x JOIN sessions s ON s.id = x.session_id
         LEFT JOIN summaries m ON m.session_id = s.id
         ORDER BY s.last_ts DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let summary = r
                .get::<_, Option<String>>(3)?
                .and_then(|b| summarize::read(&b));
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "repo": r.get::<_, String>(1)?,
                "title": title(summary.as_ref(), r.get(2)?, r.get(4)?),
                "status": summary.map(|s| s.status).unwrap_or_default(),
                "last_ts": r.get::<_, i64>(5)?,
                "source": r.get::<_, String>(6)?,
                "machine": r.get::<_, String>(7)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// このマシンでそのリポジトリに開いたセッション。新しいものから limit 件
pub fn in_repo(conn: &Connection, repo: &str, limit: i64) -> Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.title, m.body,
                (SELECT text FROM prompts p WHERE p.session_id = s.id ORDER BY seq LIMIT 1),
                s.first_ts, s.last_ts, s.source
         FROM sessions s LEFT JOIN summaries m ON m.session_id = s.id
         WHERE s.repo = ?1 AND s.machine = ''
         ORDER BY s.last_ts DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![repo, limit], |r| {
            let summary = r
                .get::<_, Option<String>>(2)?
                .and_then(|b| summarize::read(&b));
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "title": title(summary.as_ref(), r.get(1)?, r.get(3)?),
                "status": summary.map(|s| s.status).unwrap_or_default(),
                "first_ts": r.get::<_, i64>(4)?,
                "last_ts": r.get::<_, i64>(5)?,
                "source": r.get::<_, String>(6)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(conn: &Connection, id: &str, title: &str, last_ts: i64) {
        conn.execute(
            "INSERT INTO sessions VALUES (?1, ?1, 0, 0, '/r', '/r', 'main', ?2, 0, ?3, 'u', 1, 'claude', '')",
            params![id, title, last_ts],
        )
        .unwrap();
    }

    #[test]
    fn pins_and_lists_recent_first() {
        let conn = crate::db::open_at(&crate::db::temp_dir("pins").join("t.db")).unwrap();
        session(&conn, "a", "古い", 1000);
        session(&conn, "b", "", 2000);
        conn.execute(
            "INSERT INTO prompts VALUES ('b', 0, 2000, 'テストを直して')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO summaries (session_id, last_uuid, model, body, created_at) VALUES ('a', 'u', 'm', ?1, 0)",
            [r#"{"title":"要約の題","bullets":[],"status":"wip"}"#],
        )
        .unwrap();
        assert!(list(&conn).unwrap().is_empty());
        set(&conn, "a", true, 1).unwrap();
        set(&conn, "a", true, 2).unwrap(); // 二度押しても 1 つ
        set(&conn, "b", true, 3).unwrap();
        assert!(is_pinned(&conn, "a").unwrap());
        let l = list(&conn).unwrap();
        let got: Vec<_> = l
            .iter()
            .map(|v| {
                (
                    v["id"].as_str().unwrap(),
                    v["title"].as_str().unwrap(),
                    v["status"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(got, [("b", "テストを直して", ""), ("a", "要約の題", "wip")]);
        set(&conn, "a", false, 4).unwrap();
        assert!(!is_pinned(&conn, "a").unwrap());
        assert_eq!(list(&conn).unwrap().len(), 1);
    }

    #[test]
    fn sessions_in_a_repo() {
        let conn = crate::db::open_at(&crate::db::temp_dir("pins-repo").join("t.db")).unwrap();
        session(&conn, "old", "古い", 1000);
        session(&conn, "new", "新しい", 3000);
        session(&conn, "mid", "中", 2000);
        conn.execute(
            "UPDATE sessions SET machine = 'ai-node' WHERE id = 'mid'",
            [],
        )
        .unwrap();
        let l = in_repo(&conn, "/r", 10).unwrap();
        let ids: Vec<_> = l.iter().map(|v| v["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["new", "old"]); // 別のマシンのものは出さない
        assert_eq!(l[0]["title"], "新しい");
        assert_eq!(in_repo(&conn, "/r", 1).unwrap().len(), 1);
        assert!(in_repo(&conn, "/other", 10).unwrap().is_empty());
    }

    #[test]
    fn title_falls_back() {
        assert_eq!(
            title(None, "AI の題".into(), Some("依頼".into())),
            "AI の題"
        );
        assert_eq!(
            title(None, String::new(), Some("あ".repeat(50))),
            "あ".repeat(40)
        );
        assert_eq!(title(None, String::new(), None), "");
    }
}
