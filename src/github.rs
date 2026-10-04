//! リポジトリの画面: GitHub のリポジトリの一覧（gh）と、手元の clone を突き合わせる。
//! 扱う組織は設定で選んだものだけ（一覧も clone も作業の開始も）
use crate::settings::Settings;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// origin の URL → "owner/repo"（小文字）。GitHub でなければ None
pub fn slug(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = [
        "git@github.com:",
        "ssh://git@github.com/",
        "https://github.com/",
        "http://github.com/",
    ]
    .iter()
    .find_map(|p| url.strip_prefix(p))?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(format!(
        "{}/{}",
        owner.to_ascii_lowercase(),
        repo.to_ascii_lowercase()
    ))
}

/// .git/config から origin の URL を読む（フォルダの数だけ git を起動しないように）
fn origin_url(repo: &Path) -> Option<String> {
    let text = std::fs::read_to_string(repo.join(".git/config")).ok()?;
    let mut in_origin = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_origin = l == "[remote \"origin\"]";
            continue;
        }
        if in_origin
            && let Some((key, value)) = l.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Local {
    pub path: String,
    /// "owner/repo"（小文字）
    pub slug: String,
}

/// 探すフォルダの直下にある git リポジトリのうち、origin が GitHub のもの
pub fn locals(roots: &[String]) -> Vec<Local> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join(".git").is_dir())
            .collect();
        dirs.sort();
        for d in dirs {
            if let Some(slug) = origin_url(&d).as_deref().and_then(slug) {
                out.push(Local {
                    path: d.to_string_lossy().into_owned(),
                    slug,
                });
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Remote {
    pub owner: String,
    pub name: String,
    pub description: String,
    pub private: bool,
    pub archived: bool,
    pub pushed_at: String,
    pub url: String,
}

const LIST_FIELDS: &str = "name,description,isPrivate,isArchived,pushedAt,url";

fn parse_list(owner: &str, json: &str) -> Result<Vec<Remote>> {
    let v: Vec<Value> = serde_json::from_str(json).context("gh output is not a JSON list")?;
    Ok(v.iter()
        .filter_map(|r| {
            Some(Remote {
                owner: owner.to_string(),
                name: r["name"].as_str()?.to_string(),
                description: r["description"].as_str().unwrap_or("").to_string(),
                private: r["isPrivate"].as_bool().unwrap_or(false),
                archived: r["isArchived"].as_bool().unwrap_or(false),
                pushed_at: r["pushedAt"].as_str().unwrap_or("").to_string(),
                url: r["url"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect())
}

/// 組織のリポジトリの一覧。手元の `gh` のログインを使う
pub fn list(set: &Settings, owner: &str) -> Result<Vec<Remote>> {
    list_with(set, owner, &["gh"])
}

/// gh は起動するコマンド（テストでは sh にスクリプトを読ませる）
fn list_with(set: &Settings, owner: &str, gh: &[&str]) -> Result<Vec<Remote>> {
    if !set.owner_allowed(owner) {
        bail!("{owner} is not in the GitHub owners in settings");
    }
    let out = Command::new(gh[0])
        .args(&gh[1..])
        .args([
            "repo",
            "list",
            owner,
            "--limit",
            "1000",
            "--json",
            LIST_FIELDS,
        ])
        .output()
        .context("cannot start gh (install the GitHub CLI and run `gh auth login`)")?;
    if !out.status.success() {
        bail!(
            "gh repo list {owner} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse_list(owner, &String::from_utf8_lossy(&out.stdout))
}

/// リポジトリの名前として受けてよいか（GitHub の名前に使える文字だけ。. や - で始まるものは断る）
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['.', '-'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// clone する。組織は設定で選んだもの、置き場所は探すフォルダのどれか。同じ名前のフォルダがあれば断る。
/// 戻り値は clone したフォルダ
pub fn clone(set: &Settings, owner: &str, name: &str, root: &str) -> Result<String> {
    clone_with(set, owner, name, root, &["gh"])
}

fn clone_with(set: &Settings, owner: &str, name: &str, root: &str, gh: &[&str]) -> Result<String> {
    if !set.owner_allowed(owner) {
        bail!("{owner} is not in the GitHub owners in settings");
    }
    if !valid_name(name) {
        bail!("not a repository name: {name}");
    }
    if !set.repo_roots.iter().any(|r| r == root) {
        bail!("{root} is not one of the folders in settings");
    }
    let dest = Path::new(root).join(name);
    if dest.exists() {
        bail!("{} already exists", dest.display());
    }
    let out = Command::new(gh[0])
        .args(&gh[1..])
        .args(["repo", "clone", &format!("{owner}/{name}")])
        .arg(&dest)
        .output()
        .context("cannot start gh (install the GitHub CLI and run `gh auth login`)")?;
    if !out.status.success() {
        bail!(
            "gh repo clone failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(dest.to_string_lossy().into_owned())
}

/// 作業を始めてよいフォルダか: 探すフォルダの中の、設定で選んだ組織のリポジトリだけ
pub fn startable(set: &Settings, dir: &str) -> bool {
    locals(&set.repo_roots).iter().any(|l| {
        l.path == dir
            && l.slug
                .split_once('/')
                .is_some_and(|(owner, _)| set.owner_allowed(owner))
    })
}

fn git_out(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// origin の既定のブランチ（origin/HEAD が指す先）。clone したものなら分かる。分からなければ None
pub fn default_branch(dir: &str) -> Option<String> {
    git_out(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .and_then(|h| h.strip_prefix("origin/").map(str::to_string))
}

/// 「ここで始める」で出発点に選べるブランチ: origin にあるものと手元にあるもの（名前順、重複なし）。
/// 手元にある記録だけを見る（GitHub へは取りに行かない）
pub fn branches(dir: &str) -> Vec<String> {
    let refs = git_out(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes/origin",
        ],
    )
    .unwrap_or_default();
    let names: std::collections::BTreeSet<String> = refs
        .lines()
        .filter_map(|r| {
            r.strip_prefix("refs/heads/")
                .or_else(|| r.strip_prefix("refs/remotes/origin/"))
        })
        .filter(|n| *n != "HEAD")
        .map(str::to_string)
        .collect();
    names.into_iter().collect()
}

/// セッションの詳細から GitHub へ飛ぶリンク: リポジトリ、ブランチ（push 済みのときだけ）、
/// 既定のブランチとの比較（PR を作る画面。push 済みで既定のブランチ以外のときだけ）。origin が GitHub でなければ None
pub fn links(dir: &str, branch: &str) -> Option<serde_json::Value> {
    let git = |args: &[&str]| git_out(dir, args);
    let repo = format!(
        "https://github.com/{}",
        slug(&git(&["remote", "get-url", "origin"])?)?
    );
    let pushed = !branch.is_empty()
        && git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{branch}"),
        ])
        .is_some();
    // origin/HEAD が分からなければ、比較は出さない（どこと比べるか決められない）
    let default = default_branch(dir);
    let path = |b: &str| {
        b.replace('%', "%25")
            .replace('#', "%23")
            .replace(' ', "%20")
    };
    Some(serde_json::json!({
        "repo": repo,
        "branch": pushed.then(|| format!("{repo}/tree/{}", path(branch))),
        "compare": (pushed && default.as_deref().is_some_and(|d| d != branch))
            .then(|| format!("{repo}/compare/{}?expand=1", path(branch))),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        for url in [
            "git@github.com:UIUIFREE/Agent-Calendar.git",
            "https://github.com/uiuifree/agent-calendar",
            "https://github.com/uiuifree/agent-calendar.git/",
            "ssh://git@github.com/uiuifree/agent-calendar.git",
        ] {
            assert_eq!(
                slug(url).as_deref(),
                Some("uiuifree/agent-calendar"),
                "{url}"
            );
        }
        for url in [
            "git@gitlab.com:a/b.git",
            "https://github.com/a",
            "https://github.com/a/b/c",
            "https://github.com//b",
        ] {
            assert_eq!(slug(url), None, "{url}");
        }
    }

    fn git_repo(root: &Path, name: &str, config: &str) {
        let d = root.join(name).join(".git");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("config"), config).unwrap();
    }

    fn settings(root: &Path) -> Settings {
        Settings {
            github_owners: vec!["UiuiFree".into()],
            repo_roots: vec![root.to_string_lossy().into_owned()],
            ..Default::default()
        }
    }

    #[test]
    fn finds_local_clones() {
        let root = crate::db::temp_dir("gh-locals");
        git_repo(
            &root,
            "b-repo",
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@github.com:x/y.git\n[remote \"origin\"]\n\turl = git@github.com:uiuifree/b-repo.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        );
        git_repo(
            &root,
            "a-repo",
            "[remote \"origin\"]\n\turl=https://github.com/other/a-repo\n",
        );
        git_repo(
            &root,
            "gitlab",
            "[remote \"origin\"]\n\turl = git@gitlab.com:x/y.git\n",
        );
        git_repo(&root, "no-origin", "[core]\n");
        std::fs::create_dir_all(root.join("plain")).unwrap();
        let root_s = root.to_string_lossy().into_owned();
        let found = locals(&[root_s.clone(), "/nonexistent".into()]);
        let slugs: Vec<_> = found.iter().map(|l| l.slug.as_str()).collect();
        assert_eq!(slugs, ["other/a-repo", "uiuifree/b-repo"]);
        assert_eq!(found[1].path, format!("{root_s}/b-repo"));
        // 作業を始められるのは、設定で選んだ組織のリポジトリだけ
        let set = settings(&root);
        assert!(startable(&set, &found[1].path));
        assert!(!startable(&set, &found[0].path));
        assert!(!startable(&set, &format!("{root_s}/plain")));
    }

    #[test]
    fn links_to_github() {
        let dir = crate::db::temp_dir("gh-links").join("r");
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().into_owned();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&dir)
                    .args(args)
                    .status()
                    .unwrap()
                    .success(),
                "{args:?}"
            )
        };
        git(&["init", "-q"]);
        assert!(links(&d, "main").is_none()); // origin が無い
        git(&[
            "remote",
            "add",
            "origin",
            "git@github.com:uiuifree/Agent-Calendar.git",
        ]);
        let v = links(&d, "feature/x").unwrap();
        assert_eq!(v["repo"], "https://github.com/uiuifree/agent-calendar");
        assert!(v["branch"].is_null() && v["compare"].is_null()); // push していない
        git(&[
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
        git(&["update-ref", "refs/remotes/origin/feature/x", "HEAD"]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let v = links(&d, "feature/x").unwrap();
        assert_eq!(
            v["branch"],
            "https://github.com/uiuifree/agent-calendar/tree/feature/x"
        );
        assert!(v["compare"].is_null()); // 既定のブランチが分からない
        git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        let v = links(&d, "feature/x").unwrap();
        assert_eq!(
            v["compare"],
            "https://github.com/uiuifree/agent-calendar/compare/feature/x?expand=1"
        );
        // 既定のブランチそのものなら比べない
        assert!(links(&d, "main").unwrap()["compare"].is_null());
        git(&["remote", "set-url", "origin", "git@gitlab.com:a/b.git"]);
        assert!(links(&d, "main").is_none());
    }

    /// gh の代わり。受けた引数をファイルに書き、決まった出力を返す。
    /// 書いたばかりのスクリプトを直接実行すると、まれに「Text file busy」になるので sh に読ませる
    fn fake_gh(dir: &Path, name: &str, body: &str) -> String {
        let f = dir.join(format!("{name}.sh"));
        std::fs::write(
            &f,
            format!("echo \"$@\" > {}/args\n{body}\n", dir.display()),
        )
        .unwrap();
        f.to_string_lossy().into_owned()
    }

    #[test]
    fn lists_only_allowed_owners() {
        let dir = crate::db::temp_dir("gh-list");
        let set = settings(&dir);
        let ok = fake_gh(
            &dir,
            "ok",
            r#"echo '[{"name":"agent-calendar","description":null,"isPrivate":false,"isArchived":false,"pushedAt":"2026-10-04T00:00:00Z","url":"https://github.com/uiuifree/agent-calendar"},{"description":"no name"}]'"#,
        );
        let l = list_with(&set, "uiuifree", &["sh", &ok]).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(
            (
                l[0].owner.as_str(),
                l[0].name.as_str(),
                l[0].description.as_str()
            ),
            ("uiuifree", "agent-calendar", "")
        );
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        assert!(args.starts_with("repo list uiuifree --limit 1000 --json "));
        assert!(list_with(&set, "someone-else", &["sh", &ok]).is_err());
        let failing = fake_gh(&dir, "failing", "echo 'not logged in' >&2; exit 1");
        assert!(
            list_with(&set, "uiuifree", &["sh", &failing])
                .unwrap_err()
                .to_string()
                .contains("not logged in")
        );
        assert!(
            list_with(
                &set,
                "uiuifree",
                &["sh", &fake_gh(&dir, "nope", "echo nope")]
            )
            .is_err()
        );
        assert!(list_with(&set, "uiuifree", &["/nonexistent/gh"]).is_err());
    }

    #[test]
    fn clones_into_a_root_only() {
        let dir = crate::db::temp_dir("gh-clone");
        let root = dir.join("root");
        std::fs::create_dir_all(root.join("exists")).unwrap();
        let set = settings(&root);
        let r = root.to_string_lossy().into_owned();
        let ok = fake_gh(&dir, "ok", "mkdir -p \"$4\"");
        let path = clone_with(&set, "uiuifree", "agent-calendar", &r, &["sh", &ok]).unwrap();
        assert_eq!(path, format!("{r}/agent-calendar"));
        assert!(Path::new(&path).is_dir());
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        assert_eq!(
            args.trim(),
            format!("repo clone uiuifree/agent-calendar {path}")
        );
        // 断るもの
        assert!(clone_with(&set, "other", "x", &r, &["sh", &ok]).is_err());
        for bad in ["", "..", ".hidden", "-x", "a/b", "a b"] {
            assert!(
                clone_with(&set, "uiuifree", bad, &r, &["sh", &ok]).is_err(),
                "{bad}"
            );
        }
        assert!(clone_with(&set, "uiuifree", "x", "/tmp", &["sh", &ok]).is_err());
        assert!(clone_with(&set, "uiuifree", "exists", &r, &["sh", &ok]).is_err());
        assert!(
            clone_with(
                &set,
                "uiuifree",
                "y",
                &r,
                &["sh", &fake_gh(&dir, "fail", "exit 1")]
            )
            .is_err()
        );
        assert!(clone_with(&set, "uiuifree", "y", &r, &["/nonexistent/gh"]).is_err());
    }
}
