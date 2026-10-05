use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// 集計の置き場。元の記録（jsonl）が残っているうちは scan で作り直せるが、
/// Claude Code が古い記録を消したあとは、ここにしか残らない
pub fn path() -> PathBuf {
    home().join(".local/share/agent-calendar/agent-calendar.db")
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

pub fn open() -> Result<Connection> {
    open_at(&path())
}

/// 表の形の版。形を変えるときは **表を消さずに** ALTER で足し、読み直しが要るなら
/// `UPDATE sessions SET file_size = -1` で次の scan に読ませる。Claude Code は古い記録を消す（cleanupPeriodDays）ので、
/// 消した表は元の記録からは作り直せない
const VERSION: i64 = 5;

pub fn open_at(p: &Path) -> Result<Connection> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let conn = Connection::open(p).with_context(|| format!("cannot open {}", p.display()))?;
    // 記録の要約と、別のマシンの合言葉が入るので、本人だけが読めるようにする
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("cannot set permissions on {}", p.display()))?;
    }
    // 画面の読み取りと裏の scan・要約が同時に触る
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    wal(&conn)?;
    conn.execute_batch(SCHEMA)?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > VERSION {
        anyhow::bail!(
            "{} was written by a newer agent-calendar (schema {version}); this build supports up to {VERSION}",
            p.display()
        );
    }
    // 版 4: Codex の記録も入るので、どの CLI のセッションかを持つ（版 3 までは全部 Claude Code）。
    // 画面と裏の読み直しが同時に開くので、確かめて足すまでを書き込みの取引の中で行う
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let migrated = (|| -> Result<()> {
        if !has_column(&conn, "sessions", "source")? {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN source TEXT NOT NULL DEFAULT 'claude'",
                [],
            )?;
        }
        // 版 5: 別のマシン（--remote）の記録も入るので、どのマシンかを持つ。手元は空文字
        if !has_column(&conn, "sessions", "machine")? {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN machine TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        Ok(())
    })();
    conn.execute_batch(if migrated.is_ok() {
        "COMMIT"
    } else {
        "ROLLBACK"
    })?;
    migrated?;
    conn.execute_batch(&format!("PRAGMA user_version = {VERSION}"))?;
    Ok(conn)
}

/// WAL に切り替える（画面と裏の処理が同時に読み書きするため）。切り替えの命令は待ち時間の設定が効かず、
/// 同時に開くとぶつかるので、すでに WAL なら何もしない。初回にぶつかったら少し待ってやり直す
fn wal(conn: &Connection) -> Result<()> {
    for attempt in 0..50 {
        let mode: String = match conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)) {
            Ok(m) => m,
            Err(e)
                if attempt < 49
                    && e.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if mode.eq_ignore_ascii_case("wal") {
            return Ok(());
        }
        match conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0)) {
            Ok(_) => return Ok(()),
            Err(e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("the database stayed locked while switching to WAL")
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        [table, column],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id            TEXT PRIMARY KEY,
    file          TEXT NOT NULL,
    file_size     INTEGER NOT NULL, -- 本体とサブエージェントの記録の合計
    file_mtime    INTEGER NOT NULL, -- そのうち一番新しいもの
    repo          TEXT NOT NULL,   -- リポジトリの絶対パス（worktree は本体に寄せる）
    cwd           TEXT NOT NULL,
    branch        TEXT NOT NULL,
    title         TEXT NOT NULL,   -- Claude Code が付けた題（ai-title）
    first_ts      INTEGER NOT NULL, -- unix ms
    last_ts       INTEGER NOT NULL,
    last_uuid     TEXT NOT NULL,   -- 最後の記録の印（Codex は行番号）。増えていれば要約し直す
    prompt_count  INTEGER NOT NULL,
    source        TEXT NOT NULL DEFAULT 'claude', -- claude / codex
    machine       TEXT NOT NULL DEFAULT ''        -- 手元は空文字、--remote は ssh のホスト名
);
CREATE INDEX IF NOT EXISTS sessions_last ON sessions(last_ts);
-- 10 分ごとの記録件数。帯の位置と濃さ、作業時間に使う（本体の記録だけ）
CREATE TABLE IF NOT EXISTS activity (
    session_id TEXT NOT NULL,
    bucket     INTEGER NOT NULL, -- unix 秒（600 の倍数）
    n          INTEGER NOT NULL,
    PRIMARY KEY (session_id, bucket)
);
CREATE INDEX IF NOT EXISTS activity_bucket ON activity(bucket);
-- 10 分ごと・モデルごとのトークン数。kind は main（本体）/ advisor / subagent
CREATE TABLE IF NOT EXISTS usage (
    session_id TEXT NOT NULL,
    bucket     INTEGER NOT NULL,
    model      TEXT NOT NULL,
    kind       TEXT NOT NULL,
    fast       INTEGER NOT NULL,
    input      INTEGER NOT NULL,
    output     INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    cache_5m   INTEGER NOT NULL,
    cache_1h   INTEGER NOT NULL,
    PRIMARY KEY (session_id, bucket, model, kind, fast)
);
CREATE INDEX IF NOT EXISTS usage_bucket ON usage(bucket);
CREATE TABLE IF NOT EXISTS prompts (
    session_id TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    ts         INTEGER NOT NULL,
    text       TEXT NOT NULL,
    PRIMARY KEY (session_id, seq)
);
CREATE TABLE IF NOT EXISTS summaries (
    session_id TEXT PRIMARY KEY,
    last_uuid  TEXT NOT NULL,  -- 要約したときの最後の記録。増えていれば要約し直す
    model      TEXT NOT NULL,
    body       TEXT NOT NULL,  -- {title, bullets, status, next}
    created_at INTEGER NOT NULL,
    cost_usd   REAL            -- claude -p が返した total_cost_usd
);
-- 記録を取りに行く別のマシン（`agent-calendar remote add`）。token は合言葉なので DB は本人だけが読める
CREATE TABLE IF NOT EXISTS remotes (
    name        TEXT PRIMARY KEY,
    url         TEXT NOT NULL,
    token       TEXT NOT NULL,
    fingerprint TEXT NOT NULL
);
-- 画面で変える設定（自動で動かす時間帯・間隔）。key = 'auto' に JSON
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- 予定（決めた時刻に指示を実行する）。repeat は schedule::Repeat の JSON
CREATE TABLE IF NOT EXISTS schedules (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT NOT NULL,
    dir              TEXT NOT NULL,
    agent            TEXT NOT NULL,
    mode             TEXT NOT NULL,
    worktree         INTEGER NOT NULL,
    continue_session INTEGER NOT NULL,
    prompt           TEXT NOT NULL,
    repeat           TEXT NOT NULL,
    enabled          INTEGER NOT NULL,
    next_at          INTEGER,
    last_session_id  TEXT,
    created_at       INTEGER NOT NULL
);
-- 予定の実行の記録。status は running / ok / failed / missed / skipped
CREATE TABLE IF NOT EXISTS schedule_runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    schedule_id INTEGER NOT NULL,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER,
    status      TEXT NOT NULL,
    session_id  TEXT,
    summary     TEXT NOT NULL,
    branch      TEXT NOT NULL,
    changes     INTEGER
);
CREATE INDEX IF NOT EXISTS schedule_runs_by ON schedule_runs(schedule_id, started_at);
-- リポジトリ → 案件。画面で選ぶ。無ければ「未分類」
CREATE TABLE IF NOT EXISTS repo_projects (
    repo    TEXT PRIMARY KEY,
    project TEXT NOT NULL
);
-- ピン留めしたセッション（進行中の会話をすぐ開けるように、サイドバーの上に並べる）
CREATE TABLE IF NOT EXISTS pins (
    session_id TEXT PRIMARY KEY,
    pinned_at  INTEGER NOT NULL
);
";

#[cfg(test)]
pub fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-calendar-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// テストで直接実行するスクリプトを書く。このプロセスが書き込みで開くと、並行するテストがプロセスを分ける瞬間に
/// その口が子へ写り、直後の実行がまれに「Text file busy」で失敗する。別のプロセス（sh）に書かせれば写らない
#[cfg(test)]
pub fn write_script(path: &Path, text: &str) {
    use std::io::Write;
    let mut sh = std::process::Command::new("sh")
        .args(["-c", "cat > \"$0\" && chmod 755 \"$0\""])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    sh.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    assert!(sh.wait().unwrap().success());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_keeps_rows() {
        let p = temp_dir("db").join("t.db");
        let conn = open_at(&p).unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES ('t','',0,0,'','','','',0,0,'',0,'claude','')",
            [],
        )
        .unwrap();
        drop(conn);
        let conn = open_at(&p).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, VERSION);
    }

    #[test]
    fn adds_source_to_v3_db() {
        let p = temp_dir("db-v3").join("t.db");
        let old = Connection::open(&p).unwrap();
        old.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, file TEXT NOT NULL, file_size INTEGER NOT NULL,
             file_mtime INTEGER NOT NULL, repo TEXT NOT NULL, cwd TEXT NOT NULL, branch TEXT NOT NULL,
             title TEXT NOT NULL, first_ts INTEGER NOT NULL, last_ts INTEGER NOT NULL,
             last_uuid TEXT NOT NULL, prompt_count INTEGER NOT NULL);
             INSERT INTO sessions VALUES ('s','',0,0,'','','','',0,0,'',0);
             PRAGMA user_version = 3;",
        )
        .unwrap();
        drop(old);
        let conn = open_at(&p).unwrap();
        let src: String = conn
            .query_row("SELECT source FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(src, "claude");
    }

    #[test]
    fn concurrent_opens_do_not_race() {
        let p = temp_dir("db-race").join("t.db");
        Connection::open(&p)
            .unwrap()
            .execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, file TEXT NOT NULL, file_size INTEGER NOT NULL,
                 file_mtime INTEGER NOT NULL, repo TEXT NOT NULL, cwd TEXT NOT NULL, branch TEXT NOT NULL,
                 title TEXT NOT NULL, first_ts INTEGER NOT NULL, last_ts INTEGER NOT NULL,
                 last_uuid TEXT NOT NULL, prompt_count INTEGER NOT NULL);",
            )
            .unwrap();
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let p = p.clone();
                std::thread::spawn(move || open_at(&p).map(|_| ()))
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let mode: String = Connection::open(&p)
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn refuses_newer_db() {
        let p = temp_dir("db-new").join("t.db");
        Connection::open(&p)
            .unwrap()
            .execute_batch("PRAGMA user_version = 99")
            .unwrap();
        assert!(open_at(&p).is_err());
    }
}
