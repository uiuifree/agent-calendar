//! 完了の印: 要約が「途中」のままでも、残りを片付けたセッションに手で付ける。
//! 付けた時刻を覚えておき、それより後の記録が入ったら印は効かなくなる（要約の判定に戻る）。
//! 記録は 5 分ごとの読み直しで入るので、最後の記録の印ではなく時刻で比べる（付ける前の記録が後から入っても外れない）
use crate::{pins, summarize};
use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Serialize;

/// sessions を s として引いたときに、印が効いているか（SQL の式）
pub const SQL: &str =
    "EXISTS (SELECT 1 FROM finished f WHERE f.session_id = s.id AND s.last_ts <= f.finished_at)";

/// 付ける・外す（何度押しても同じ）。そのセッションが無ければ false
pub fn set(conn: &Connection, id: &str, on: bool, now_ms: i64) -> Result<bool> {
    if !on {
        conn.execute("DELETE FROM finished WHERE session_id = ?1", [id])?;
        return Ok(true);
    }
    let n = conn.execute(
        "INSERT OR REPLACE INTO finished (session_id, finished_at)
         SELECT id, ?2 FROM sessions WHERE id = ?1",
        params![id, now_ms],
    )?;
    Ok(n > 0)
}

pub fn is_finished(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn
        .prepare(&format!(
            "SELECT 1 FROM sessions s WHERE s.id = ?1 AND {SQL}"
        ))?
        .exists([id])?)
}

/// 画面に出す状態。印が効いていれば完了、そうでなければ要約の判定
pub fn status(summary: Option<&summarize::Summary>, finished: bool) -> String {
    if finished {
        return "done".into();
    }
    summary.map(|s| s.status.clone()).unwrap_or_default()
}

/// 残タスクのあるセッション（要約が途中か、完了以外で残っていることが書かれていて、印の付いていないもの）
#[derive(Debug, PartialEq, Serialize)]
pub struct Todo {
    pub id: String,
    pub repo: String,
    /// 手元は空文字。別のマシンは `remote add` の名前
    pub machine: String,
    pub title: String,
    pub status: String,
    pub next: String,
    pub last_ts: i64,
}

/// from・to（unix 秒）の間に動きがあったセッションの残タスク（古い順）と、まだ要約していないセッションの数。
/// machines を渡すとそのホストのものだけ
pub fn todo(
    conn: &Connection,
    from: i64,
    to: i64,
    machines: Option<&[String]>,
) -> Result<(Vec<Todo>, usize)> {
    let mut stmt = conn.prepare(&format!(
        "SELECT s.id, s.repo, s.title, m.body,
                (SELECT text FROM prompts p WHERE p.session_id = s.id ORDER BY seq LIMIT 1), s.last_ts, s.machine
         FROM sessions s LEFT JOIN summaries m ON m.session_id = s.id
         WHERE NOT {SQL}
           AND EXISTS (SELECT 1 FROM activity a WHERE a.session_id = s.id AND a.bucket >= ?1 AND a.bucket < ?2)
         ORDER BY s.last_ts"
    ))?;
    let mut rows = stmt.query([from, to])?;
    let (mut list, mut unsummarized) = (Vec::new(), 0);
    while let Some(r) = rows.next()? {
        let machine: String = r.get(6)?;
        if machines.is_some_and(|m| !m.contains(&machine)) {
            continue;
        }
        let Some(summary) = r
            .get::<_, Option<String>>(3)?
            .and_then(|b| summarize::read(&b))
        else {
            unsummarized += 1;
            continue;
        };
        // 要約が完了なら、残っていることが書かれていても出さない
        if summary.status == "done" || (summary.status != "wip" && summary.next.is_empty()) {
            continue;
        }
        list.push(Todo {
            id: r.get(0)?,
            repo: r.get(1)?,
            machine,
            title: pins::title(Some(&summary), r.get(2)?, r.get(4)?),
            status: summary.status,
            next: summary.next,
            last_ts: r.get(5)?,
        });
    }
    Ok((list, unsummarized))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(conn: &Connection, id: &str, bucket: i64, body: Option<&str>) {
        conn.execute(
            "INSERT INTO sessions VALUES (?1, ?1, 0, 0, '/r/app', '/r/app', 'main', '', ?2, ?2, 'u1', 1, 'claude', '')",
            params![id, bucket * 1000],
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

    #[test]
    fn lists_what_is_left_until_marked() {
        let conn = crate::db::open_at(&crate::db::temp_dir("finished").join("t.db")).unwrap();
        session(
            &conn,
            "wip",
            1200,
            Some(r#"{"title":"途中の題","bullets":[],"status":"途中"}"#),
        );
        session(
            &conn,
            "left",
            600,
            Some(r#"{"title":"残りあり","bullets":[],"status":"相談のみ","next":"push する"}"#),
        );
        session(
            &conn,
            "done",
            1800,
            Some(r#"{"title":"済み","bullets":[],"status":"完了","next":"確認だけ残っている"}"#),
        );
        session(&conn, "raw", 2400, None);
        session(
            &conn,
            "later",
            9000,
            Some(r#"{"title":"範囲の外","bullets":[],"status":"wip"}"#),
        );

        let (list, unsummarized) = todo(&conn, 0, 3000, None).unwrap();
        let got: Vec<_> = list
            .iter()
            .map(|x| (x.id.as_str(), x.status.as_str(), x.next.as_str()))
            .collect();
        assert_eq!(got, [("left", "talk", "push する"), ("wip", "wip", "")]);
        assert_eq!(
            (
                list[1].title.as_str(),
                list[1].repo.as_str(),
                list[1].last_ts
            ),
            ("途中の題", "/r/app", 1_200_000)
        );
        assert_eq!(unsummarized, 1);
        conn.execute(
            "UPDATE sessions SET machine = 'ai-node' WHERE id = 'left'",
            [],
        )
        .unwrap();
        let only = |m: &str| todo(&conn, 0, 3000, Some(&[m.to_string()])).unwrap();
        let (list, unsummarized) = only("ai-node");
        assert_eq!(
            (list.len(), list[0].machine.as_str(), unsummarized),
            (1, "ai-node", 0)
        );
        assert_eq!(
            only("").0.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["wip"]
        );

        // 時刻は ms。セッションの最後の記録（wip は 1_200_000）より後に付ける
        assert!(set(&conn, "wip", true, 3_000_000).unwrap());
        assert!(set(&conn, "wip", true, 3_000_001).unwrap()); // 二度押しても 1 つ
        assert!(set(&conn, "raw", true, 3_000_000).unwrap());
        assert!(!set(&conn, "nope", true, 3_000_000).unwrap());
        assert!(is_finished(&conn, "wip").unwrap());
        let (list, unsummarized) = todo(&conn, 0, 3000, None).unwrap();
        assert_eq!(
            list.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["left"]
        );
        assert_eq!(unsummarized, 0);

        // 付ける前の記録が、あとの読み直しで入っても外れない
        conn.execute(
            "UPDATE sessions SET last_uuid = 'u2', last_ts = 2_900_000 WHERE id = 'wip'",
            [],
        )
        .unwrap();
        assert!(is_finished(&conn, "wip").unwrap());
        // 印を付けたあとにセッションが動いたら、要約の判定に戻る
        conn.execute(
            "UPDATE sessions SET last_ts = 3_100_000 WHERE id = 'wip'",
            [],
        )
        .unwrap();
        assert!(!is_finished(&conn, "wip").unwrap());
        assert_eq!(todo(&conn, 0, 3000, None).unwrap().0.len(), 2);

        assert!(set(&conn, "raw", false, 3_200_000).unwrap());
        assert!(!is_finished(&conn, "raw").unwrap());
    }

    #[test]
    fn status_prefers_the_mark() {
        let s = summarize::read(r#"{"title":"t","bullets":[],"status":"wip"}"#).unwrap();
        assert_eq!(status(Some(&s), false), "wip");
        assert_eq!(status(Some(&s), true), "done");
        assert_eq!(status(None, true), "done");
        assert_eq!(status(None, false), "");
    }
}
