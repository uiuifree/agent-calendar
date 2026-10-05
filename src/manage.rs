//! リポジトリごとの小さな操作: 使い終わった作業場所（git worktree）と手元のブランチの片付け、GitHub からの取り直し。
//! 手元のリポジトリだけを触る（GitHub 側のブランチは消さない）。変更が残っているものは git が断るので、そのまま断る
use crate::schedule;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::process::Command;

/// git を動かす。断られた理由を画面で読み分けるので、メッセージは英語に固定する。
/// 認証を聞かれて止まらないように、端末での問い合わせはさせない
/// 戻り値は (標準出力, 標準エラー)
fn run(dir: &str, args: &[&str]) -> Result<(String, String)> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .output()
        .context("cannot run git")?;
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        bail!("{err}");
    }
    Ok((String::from_utf8_lossy(&out.stdout).into_owned(), err))
}

fn git(dir: &str, args: &[&str]) -> Result<String> {
    run(dir, args).map(|(out, _)| out)
}

/// 手元のブランチ
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Branch {
    pub branch: String,
    /// チェックアウトしているフォルダ（本体の作業コピーか作業場所）。どこにも出ていなければ None。
    /// 出ているブランチは git が消させない
    pub checked_out: Option<String>,
}

/// 手元のブランチの一覧（名前順）
pub fn branches(dir: &str) -> Result<Vec<Branch>> {
    let out = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname:short)%09%(worktreepath)",
            "refs/heads",
        ],
    )?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let (branch, path) = l.split_once('\t')?;
            Some(Branch {
                branch: branch.to_string(),
                checked_out: (!path.is_empty()).then(|| path.to_string()),
            })
        })
        .collect())
}

/// 予定用の作業場所のブランチ（`agent-calendar/schedule-<予定の ID>`）なら、その予定の ID
pub fn schedule_of(branch: &str) -> Option<i64> {
    branch
        .strip_prefix("agent-calendar/schedule-")?
        .parse()
        .ok()
}

/// 作業場所を消す。`path` はそのリポジトリの作業場所として git が挙げるものだけを受ける。
/// 変更が残っていれば git が断る（無理には消さない）。ブランチは残る
pub fn remove_worktree(dir: &str, path: &str) -> Result<()> {
    if !schedule::worktrees(dir).iter().any(|w| w.path == path) {
        bail!("{path} is not a worktree of {dir}");
    }
    git(dir, &["worktree", "remove", path]).map(drop)
}

/// 手元のブランチを消す。まだ取り込まれていない commit があれば git が断る（force なら消す）。
/// チェックアウトしているブランチは force でも消えない
pub fn delete_branch(dir: &str, branch: &str, force: bool) -> Result<()> {
    if !schedule::valid_branch(branch) {
        bail!("not a valid branch name: {branch}");
    }
    let flag = if force { "-D" } else { "-d" };
    git(dir, &["branch", flag, branch]).map(drop)
}

/// GitHub から取り直す。GitHub で消えたブランチの手元の記録（origin/…）も片付ける。戻り値は git の報告
pub fn fetch(dir: &str) -> Result<String> {
    // fetch は進み具合も結果も標準エラーに書く
    run(dir, &["fetch", "--prune", "origin"]).map(|(_, report)| report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn cleans_up_worktrees_and_branches() {
        let dir = crate::db::temp_dir("manage");
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let r = repo.to_str().unwrap();
        let commit = |at: &str, msg: &str| {
            git(
                at,
                &[
                    "-c",
                    "user.email=a@b",
                    "-c",
                    "user.name=a",
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    msg,
                ],
            )
            .unwrap();
        };
        git(r, &["init", "-q", "-b", "main"]).unwrap();
        commit(r, "x");
        let wbase = dir.join("wts");
        let (merged, _) = schedule::new_worktree(r, "feature/merged", "", &wbase).unwrap();
        let (ahead, _) = schedule::new_worktree(r, "feature/ahead", "", &wbase).unwrap();
        let (dirty, _) = schedule::new_worktree(r, "feature/dirty", "", &wbase).unwrap();
        commit(&ahead, "y"); // main に取り込まれていない commit
        std::fs::write(Path::new(&dirty).join("left.txt"), "作業の途中").unwrap();

        // 一覧: どこに出ているブランチかが分かる
        let list = branches(r).unwrap();
        let at = |name: &str| {
            list.iter()
                .find(|b| b.branch == name)
                .unwrap()
                .checked_out
                .clone()
        };
        assert_eq!(list.len(), 4);
        assert_eq!(at("main").as_deref(), Some(r));
        assert_eq!(at("feature/merged"), Some(merged.clone()));

        // 作業場所: 変更が残っているもの・そのリポジトリの作業場所でないフォルダは消さない
        assert!(remove_worktree(r, &dirty).is_err());
        assert!(Path::new(&dirty).join("left.txt").exists());
        assert!(remove_worktree(r, dir.to_str().unwrap()).is_err());
        assert!(remove_worktree(r, r).is_err()); // 本体の作業コピー
        // 出ているブランチは消せない。作業場所を消すとブランチは残り、消せるようになる
        assert!(delete_branch(r, "feature/merged", true).is_err());
        remove_worktree(r, &merged).unwrap();
        assert!(!Path::new(&merged).exists());
        assert_eq!(
            branches(r)
                .unwrap()
                .iter()
                .find(|b| b.branch == "feature/merged"),
            Some(&Branch {
                branch: "feature/merged".into(),
                checked_out: None
            })
        );
        delete_branch(r, "feature/merged", false).unwrap();
        // 取り込まれていない commit があるブランチは、force のときだけ消す
        remove_worktree(r, &ahead).unwrap();
        let e = delete_branch(r, "feature/ahead", false).unwrap_err();
        assert!(e.to_string().contains("not fully merged"), "{e}");
        delete_branch(r, "feature/ahead", true).unwrap();
        assert!(delete_branch(r, "main", true).is_err()); // いまいるブランチ
        assert!(delete_branch(r, "-x", false).is_err());
        assert!(delete_branch(r, "nope", false).is_err());
        let left: Vec<String> = branches(r).unwrap().into_iter().map(|b| b.branch).collect();
        assert_eq!(left, ["feature/dirty", "main"]);

        // 取り直し: origin が無ければ失敗を返す。あれば、消えたブランチの記録も片付ける
        assert!(fetch(r).is_err());
        let origin = dir.join("origin.git");
        git(r, &["clone", "-q", "--bare", r, origin.to_str().unwrap()]).unwrap();
        git(r, &["remote", "add", "origin", origin.to_str().unwrap()]).unwrap();
        fetch(r).unwrap();
        let has = |name: &str| git(r, &["rev-parse", "--verify", "--quiet", name]).is_ok();
        assert!(has("refs/remotes/origin/feature/dirty"));
        git(origin.to_str().unwrap(), &["branch", "-D", "feature/dirty"]).unwrap();
        fetch(r).unwrap();
        assert!(!has("refs/remotes/origin/feature/dirty"));

        assert_eq!(schedule_of("agent-calendar/schedule-7"), Some(7));
        assert_eq!(schedule_of("agent-calendar/schedule-x"), None);
        assert_eq!(schedule_of("feature/x"), None);
    }
}
