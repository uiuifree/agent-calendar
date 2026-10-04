use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 作業ディレクトリをリポジトリ単位に寄せる。
/// worktree（.worktrees/… や別ディレクトリの acme-TICKET-123 など）は git の本体に寄せ、
/// 消えたディレクトリは名前から推測する
#[derive(Default)]
pub struct Resolver {
    cache: HashMap<String, String>,
}

impl Resolver {
    pub fn resolve(&mut self, cwd: &str) -> String {
        if let Some(r) = self.cache.get(cwd) {
            return r.clone();
        }
        let r = resolve_uncached(cwd);
        self.cache.insert(cwd.to_string(), r.clone());
        r
    }
}

fn resolve_uncached(cwd: &str) -> String {
    if Path::new(cwd).is_dir() {
        return git_root(cwd).unwrap_or_else(|| cwd.to_string());
    }
    // 消えた worktree: 本体側のディレクトリがあればそこへ
    for marker in ["/.worktrees/", "/.claude/worktrees/"] {
        if let Some(i) = cwd.find(marker) {
            let base = &cwd[..i];
            if Path::new(base).is_dir() {
                return git_root(base).unwrap_or_else(|| base.to_string());
            }
            return base.to_string();
        }
    }
    // 消えた枝別ディレクトリ（foo-TICKET-123）: foo があればそこへ
    if let Some(base) = strip_ticket_suffix(cwd)
        && Path::new(&base).is_dir()
    {
        return git_root(&base).unwrap_or(base);
    }
    cwd.to_string()
}

fn git_root(dir: &str) -> Option<String> {
    let out = Command::new("git")
        .args([
            "-C",
            dir,
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    // 本体の .git を指すので、その親がリポジトリ
    let root = if common.file_name().is_some_and(|n| n == ".git") {
        common.parent()?.to_path_buf()
    } else {
        common
    };
    Some(root.to_string_lossy().into_owned())
}

/// 末尾の `-ABC-123` を外す
fn strip_ticket_suffix(path: &str) -> Option<String> {
    let (head, num) = path.rsplit_once('-')?;
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (base, key) = head.rsplit_once('-')?;
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    Some(base.to_string())
}

/// 別のマシンの作業ディレクトリ。手元に無いので git には聞けず、worktree の部分を外すだけ
pub fn without_worktree(cwd: &str) -> String {
    for marker in ["/.worktrees/", "/.claude/worktrees/"] {
        if let Some(i) = cwd.find(marker) {
            return cwd[..i].to_string();
        }
    }
    cwd.to_string()
}

pub fn name(repo: &str) -> &str {
    repo.rsplit('/').find(|s| !s.is_empty()).unwrap_or(repo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ticket_suffix() {
        assert_eq!(
            strip_ticket_suffix("/home/u/src/acme-web-TICKET-123").as_deref(),
            Some("/home/u/src/acme-web")
        );
        assert_eq!(strip_ticket_suffix("/home/u/min-wage-2"), None);
        assert_eq!(strip_ticket_suffix("/home/u/api-v2"), None);
    }

    #[test]
    fn missing_worktree_falls_back_to_base() {
        assert_eq!(
            resolve_uncached("/nonexistent/repo/.worktrees/feature-x"),
            "/nonexistent/repo"
        );
    }

    #[test]
    fn resolves_worktree_to_main_repo() {
        let dir = crate::db::temp_dir("repo");
        let main = dir.join("main");
        std::fs::create_dir_all(&main).unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&main)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "-q"]);
        run(&[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        let wt = dir.join("main-TICKET-1");
        run(&["worktree", "add", "-q", wt.to_str().unwrap()]);
        let main_s = main.to_string_lossy().into_owned();
        let mut r = Resolver::default();
        assert_eq!(r.resolve(wt.to_str().unwrap()), main_s);
        assert_eq!(r.resolve(wt.to_str().unwrap()), main_s); // 2 回目はキャッシュ
        // git でないディレクトリはそのまま
        assert_eq!(r.resolve(dir.to_str().unwrap()), dir.to_string_lossy());
        // 消えた枝別ディレクトリ・worktree は本体へ
        assert_eq!(r.resolve(&format!("{main_s}-TICKET-999")), main_s);
        assert_eq!(r.resolve(&format!("{main_s}/.worktrees/gone")), main_s);
        assert_eq!(r.resolve("/nonexistent/x"), "/nonexistent/x");
    }

    #[test]
    fn remote_worktrees_fold_by_path() {
        assert_eq!(without_worktree("/h/u/app/.claude/worktrees/x"), "/h/u/app");
        assert_eq!(without_worktree("/h/u/app/.worktrees/y/z"), "/h/u/app");
        assert_eq!(without_worktree("/h/u/app"), "/h/u/app");
    }

    #[test]
    fn name_is_last_segment() {
        assert_eq!(name("/home/u/src/acme-api"), "acme-api");
        assert_eq!(name("/mnt/d/notes/"), "notes");
    }
}
