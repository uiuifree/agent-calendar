//! 自動で動かす時間帯と間隔、リポジトリの画面で扱う GitHub の組織と探すフォルダ。画面の設定から変えられるように DB に置く
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 自動で動かす時間帯（この PC の時刻）。start <= 時 < end。start > end なら日をまたぐ（22〜6 など）。
    /// start == end なら一日中
    pub start_hour: u32,
    pub end_hour: u32,
    /// 記録の読み直し（別のマシンからの取り込みを含む）の間隔
    pub refresh_minutes: u32,
    /// 要約の間隔（1 回あたりの件数は固定）
    pub summary_minutes: u32,
    /// リポジトリの画面で扱う GitHub の組織・ユーザー。ここに無い組織の一覧・clone・作業開始はしない
    #[serde(default)]
    pub github_owners: Vec<String>,
    /// 手元のリポジトリを探すフォルダ（直下の git リポジトリを見る）。clone もこの中に置く
    #[serde(default)]
    pub repo_roots: Vec<String>,
}

/// 組織・フォルダはそれぞれこの数まで
const MAX_LIST: usize = 20;

impl Default for Settings {
    fn default() -> Self {
        Settings {
            start_hour: 7,
            end_hour: 22,
            refresh_minutes: 5,
            summary_minutes: 60,
            github_owners: Vec::new(),
            repo_roots: Vec::new(),
        }
    }
}

impl Settings {
    pub fn active_at(&self, hour: u32) -> bool {
        match self.start_hour.cmp(&self.end_hour) {
            std::cmp::Ordering::Equal => true,
            std::cmp::Ordering::Less => self.start_hour <= hour && hour < self.end_hour,
            std::cmp::Ordering::Greater => hour >= self.start_hour || hour < self.end_hour,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.start_hour > 23 || self.end_hour > 24 {
            bail!("hours must be 0-23 (start) and 0-24 (end)");
        }
        for (name, m) in [
            ("refresh_minutes", self.refresh_minutes),
            ("summary_minutes", self.summary_minutes),
        ] {
            if !(1..=1440).contains(&m) {
                bail!("{name} must be between 1 and 1440");
            }
        }
        if self.github_owners.len() > MAX_LIST || self.repo_roots.len() > MAX_LIST {
            bail!("up to {MAX_LIST} owners and {MAX_LIST} folders");
        }
        let mut seen = std::collections::HashSet::new();
        for o in &self.github_owners {
            // GitHub の名前は英数字とハイフンだけ（39 文字まで）
            let ok = !o.is_empty()
                && o.len() <= 39
                && o.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !ok {
                bail!("not a GitHub owner name: {o}");
            }
            if !seen.insert(o.to_ascii_lowercase()) {
                bail!("{o} is listed twice");
            }
        }
        for r in &self.repo_roots {
            let p = std::path::Path::new(r);
            if !p.is_absolute() || !p.is_dir() {
                bail!("not an existing folder (use an absolute path): {r}");
            }
            if !seen.insert(r.clone()) {
                bail!("{r} is listed twice");
            }
        }
        Ok(())
    }

    /// 設定で選んだ組織か（GitHub の名前は大文字・小文字を区別しない）
    pub fn owner_allowed(&self, owner: &str) -> bool {
        self.github_owners
            .iter()
            .any(|o| o.eq_ignore_ascii_case(owner))
    }
}

/// 前回から `minutes` 以上たったか（まだ一度も動いていなければ true）
pub fn due(last: Option<std::time::Instant>, minutes: u32, now: std::time::Instant) -> bool {
    last.is_none_or(|t| now.duration_since(t).as_secs() >= u64::from(minutes) * 60)
}

pub fn load(conn: &Connection) -> Result<Settings> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = 'auto'", [], |r| {
            r.get(0)
        })
        .optional()?;
    // 壊れていたり項目が足りなかったりしたら既定に戻す（画面から保存し直せば直る）
    Ok(v.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

pub fn save(conn: &Connection, s: &Settings) -> Result<()> {
    s.validate()?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('auto', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [serde_json::to_string(s)?],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn s(start: u32, end: u32) -> Settings {
        Settings {
            start_hour: start,
            end_hour: end,
            ..Default::default()
        }
    }

    #[test]
    fn hours_window() {
        let day = s(7, 22);
        assert!(!day.active_at(6) && day.active_at(7) && day.active_at(21) && !day.active_at(22));
        let night = s(22, 6);
        assert!(
            night.active_at(23)
                && night.active_at(0)
                && night.active_at(5)
                && !night.active_at(6)
                && !night.active_at(12)
        );
        assert!(s(0, 0).active_at(3) && s(0, 24).active_at(23));
    }

    #[test]
    fn interval_due() {
        let now = Instant::now();
        assert!(due(None, 5, now));
        assert!(!due(Some(now), 5, now + Duration::from_secs(299)));
        assert!(due(Some(now), 5, now + Duration::from_secs(300)));
    }

    #[test]
    fn saves_and_validates() {
        let conn = crate::db::open_at(&crate::db::temp_dir("settings").join("t.db")).unwrap();
        assert_eq!(load(&conn).unwrap(), Settings::default());
        let root = crate::db::temp_dir("settings-root");
        let mine = Settings {
            start_hour: 8,
            end_hour: 20,
            refresh_minutes: 10,
            summary_minutes: 120,
            github_owners: vec!["uiuifree".into(), "my-org".into()],
            repo_roots: vec![root.to_string_lossy().into_owned()],
        };
        save(&conn, &mine).unwrap();
        assert_eq!(load(&conn).unwrap(), mine);
        for bad in [
            Settings {
                start_hour: 24,
                ..mine.clone()
            },
            Settings {
                end_hour: 25,
                ..mine.clone()
            },
            Settings {
                refresh_minutes: 0,
                ..mine.clone()
            },
            Settings {
                summary_minutes: 1441,
                ..mine.clone()
            },
            Settings {
                github_owners: vec!["evil/../x".into()],
                ..mine.clone()
            },
            Settings {
                github_owners: vec!["UIUIFREE".into(), "uiuifree".into()],
                ..mine.clone()
            },
            Settings {
                github_owners: vec![String::new()],
                ..mine.clone()
            },
            Settings {
                github_owners: vec!["a".repeat(40)],
                ..mine.clone()
            },
            Settings {
                github_owners: (0..21).map(|i| format!("o{i}")).collect(),
                ..mine.clone()
            },
            Settings {
                repo_roots: vec!["relative/dir".into()],
                ..mine.clone()
            },
            Settings {
                repo_roots: vec!["/nonexistent/dir".into()],
                ..mine.clone()
            },
            Settings {
                repo_roots: vec![mine.repo_roots[0].clone(), mine.repo_roots[0].clone()],
                ..mine.clone()
            },
        ] {
            assert!(save(&conn, &bad).is_err());
        }
        assert_eq!(load(&conn).unwrap(), mine);
        // 前の版で保存した設定（組織とフォルダの項目が無い）もそのまま読める
        conn.execute(
            "UPDATE settings SET value = '{\"start_hour\":9,\"end_hour\":18,\"refresh_minutes\":5,\"summary_minutes\":60}'",
            [],
        )
        .unwrap();
        let old = load(&conn).unwrap();
        assert_eq!((old.start_hour, old.github_owners.len()), (9, 0));
        conn.execute("UPDATE settings SET value = 'broken'", [])
            .unwrap();
        assert_eq!(load(&conn).unwrap(), Settings::default());
    }

    #[test]
    fn owners_are_case_insensitive() {
        let s = Settings {
            github_owners: vec!["My-Org".into()],
            ..Default::default()
        };
        assert!(s.owner_allowed("my-org") && !s.owner_allowed("other"));
    }
}
