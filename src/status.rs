//! 裏の処理（読み直し・要約・別マシンの取り込み）の最後の結果。失敗しても画面に出ないと、
//! 止まっていることに気づけないので、最後にうまくいった時刻と最後の失敗の理由を残す
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

/// 結果を残す。error が None ならうまくいった（前の失敗は消す）
pub fn set(conn: &Connection, name: &str, error: Option<&str>) -> Result<()> {
    let now = chrono::Utc::now().timestamp_millis();
    match error {
        None => conn.execute(
            "INSERT INTO job_status (name, ok_at) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET ok_at = ?2, error = NULL, error_at = NULL",
            params![name, now],
        )?,
        Some(e) => conn.execute(
            "INSERT INTO job_status (name, error, error_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET error = ?2, error_at = ?3",
            params![name, e, now],
        )?,
    };
    Ok(())
}

/// 全部（画面用）
pub fn all(conn: &Connection) -> Result<Vec<Value>> {
    let mut stmt =
        conn.prepare("SELECT name, ok_at, error, error_at FROM job_status ORDER BY name")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "name": r.get::<_, String>(0)?,
                "ok_at": r.get::<_, Option<i64>>(1)?,
                "error": r.get::<_, Option<String>>(2)?,
                "error_at": r.get::<_, Option<i64>>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_last_ok_and_error() {
        let conn = crate::db::open_at(&crate::db::temp_dir("status").join("t.db")).unwrap();
        set(&conn, "scan", None).unwrap();
        set(&conn, "remote:pi", Some("connection refused")).unwrap();
        let v = all(&conn).unwrap();
        assert_eq!(v[0]["name"], "remote:pi");
        assert_eq!(v[0]["error"], "connection refused");
        assert!(v[0]["ok_at"].is_null());
        assert!(v[1]["ok_at"].is_i64() && v[1]["error"].is_null());
        // うまくいったら失敗の理由は消える。失敗しても前にうまくいった時刻は残る
        set(&conn, "remote:pi", None).unwrap();
        set(&conn, "scan", Some("could not read 1 files")).unwrap();
        let v = all(&conn).unwrap();
        assert!(v[0]["error"].is_null() && v[0]["ok_at"].is_i64());
        assert_eq!(v[1]["error"], "could not read 1 files");
        assert!(v[1]["ok_at"].is_i64());
    }
}
