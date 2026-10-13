//! 会話から「新しいセッションで始める」ときの作業場所。そのセッションが動いたフォルダを含む作業場所
//! （リポジトリの本体か git の worktree。中のフォルダで動いていたら、いちばん近いもの）と、いまそこに出ているブランチ。
//! フォルダは画面から受け取らず、このマシンの記録から決める（記録にあるフォルダでしか始めない）
use crate::{manage, schedule};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, PartialEq, Serialize)]
pub struct Place {
    pub path: String,
    /// いま出ているブランチ。ブランチの上にいない（detached）か git でなければ空
    pub branch: String,
    /// リポジトリの本体か（でなければ worktree）
    pub main: bool,
}

/// 作業場所を選ぶ。動いたフォルダが消えていれば（中に置いた worktree を消したときも）選ばない
fn pick(
    cwd: &str,
    repo: &str,
    worktrees: &[schedule::Worktree],
    branches: &[manage::Branch],
) -> Option<Place> {
    if !Path::new(cwd).is_dir() {
        return None;
    }
    let path = std::iter::once(repo)
        .chain(worktrees.iter().map(|w| w.path.as_str()))
        .filter(|p| !p.is_empty() && (cwd == *p || cwd.starts_with(&format!("{p}/"))))
        .max_by_key(|p| p.len())?;
    let branch = branches
        .iter()
        .find(|b| b.checked_out.as_deref() == Some(path))
        .map(|b| b.branch.clone())
        .unwrap_or_default();
    Some(Place {
        path: path.to_string(),
        main: path == repo,
        branch,
    })
}

/// このマシンのセッションの (作業ディレクトリ, リポジトリ)。セッションが無ければ Err
pub fn dirs(conn: &Connection, id: &str) -> Result<(String, String)> {
    conn.query_row(
        "SELECT cwd, repo FROM sessions WHERE id = ?1 AND machine = ''",
        [id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .optional()?
    .ok_or_else(|| anyhow::anyhow!("no such session on this machine"))
}

/// その作業ディレクトリの作業場所（git を動かす）。消えていれば None
pub fn find(cwd: &str, repo: &str) -> Option<Place> {
    let worktrees = schedule::worktrees(repo);
    // git でないフォルダならブランチは無い
    let branches = manage::branches(repo).unwrap_or_default();
    pick(cwd, repo, &worktrees, &branches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_nearest_folder_and_its_branch() {
        let d = crate::db::temp_dir("place");
        let r = d.join("r");
        let wt = d.join("wt-a");
        let inner = r.join(".wt/b");
        for p in [
            &r,
            &wt,
            &inner,
            &r.join("web"),
            &wt.join("src"),
            &d.join("r2"),
        ] {
            std::fs::create_dir_all(p).unwrap();
        }
        let (r, wt, inner) = (
            r.to_str().unwrap(),
            wt.to_str().unwrap(),
            inner.to_str().unwrap(),
        );
        let worktrees = [
            schedule::Worktree {
                path: wt.into(),
                branch: "feature/a".into(),
            },
            schedule::Worktree {
                path: inner.into(),
                branch: String::new(),
            },
            schedule::Worktree {
                path: format!("{r}-gone"),
                branch: "x".into(),
            },
        ];
        let branches = [
            manage::Branch {
                branch: "main".into(),
                checked_out: Some(r.into()),
            },
            manage::Branch {
                branch: "feature/a".into(),
                checked_out: Some(wt.into()),
            },
            manage::Branch {
                branch: "old".into(),
                checked_out: None,
            },
        ];
        let at = |cwd: &str| pick(cwd, r, &worktrees, &branches);
        let place = |path: &str, branch: &str, main| {
            Some(Place {
                path: path.into(),
                branch: branch.into(),
                main,
            })
        };
        assert_eq!(at(wt), place(wt, "feature/a", false));
        assert_eq!(at(&format!("{wt}/src")), place(wt, "feature/a", false));
        assert_eq!(at(r), place(r, "main", true));
        assert_eq!(at(&format!("{r}/web")), place(r, "main", true));
        // 本体の中に置いた worktree はそちら。ブランチに出ていなければ空
        assert_eq!(at(inner), place(inner, "", false));
        // 消えた作業場所・名前の前だけが同じフォルダは選ばない
        assert_eq!(at(&format!("{r}-gone")), None);
        assert_eq!(at(&format!("{r}2")), None);
        // 本体の中に置いた worktree を消したら、本体では始めない
        assert_eq!(at(&format!("{r}/.wt/removed")), None);
        assert_eq!(pick(r, &format!("{r}-none"), &[], &[]), None);
    }

    #[test]
    fn reads_the_session_from_the_records() {
        let d = crate::db::temp_dir("place-db");
        let conn = crate::db::open_at(&d.join("t.db")).unwrap();
        let here = d.to_str().unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES ('s', 's', 0, 0, ?1, ?1, '', '', 0, 0, 'u', 1, 'claude', '')",
            [here],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES ('r', 'r', 0, 0, ?1, ?1, '', '', 0, 0, 'u', 1, 'claude', 'ai-node')",
            [here],
        )
        .unwrap();
        let (cwd, repo) = dirs(&conn, "s").unwrap();
        // git でないフォルダでも、そのフォルダで始める
        assert_eq!(
            find(&cwd, &repo),
            Some(Place {
                path: here.into(),
                branch: String::new(),
                main: true
            })
        );
        assert!(dirs(&conn, "r").is_err()); // 別のマシン
        assert!(dirs(&conn, "nope").is_err());
    }
}
