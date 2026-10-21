//! セッションの「変更」タブ: まだ commit していない変更と、コミットの差分を git から読む（見るだけ）
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::process::Command;

/// 1 ファイルの差分の上限（これを超えたら切って、切ったことを知らせる）
const MAX_PATCH_BYTES: usize = 400 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct File {
    pub path: String,
    /// 増えた行・減った行。バイナリは None
    pub added: Option<u64>,
    pub removed: Option<u64>,
    /// まだ git に入っていない新しいファイル
    pub untracked: bool,
}

/// パスは文字のまま比べる（`*` や `:(…)` を git に解釈させない）
fn git(dir: &str, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("cannot run git")
}

fn git_ok(dir: &str, args: &[&str]) -> Result<String> {
    let out = git(dir, args)?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git diff --numstat -z --no-renames` の出力（`増\t減\tパス\0` の並び。バイナリは `-\t-\tパス`）。
/// -z だと日本語などのファイル名も引用・エスケープされず、実際のパスのまま出る。
/// 名前の変更は --no-renames で削除と追加に分けて、パスを 1 つに保つ
fn numstat(text: &str) -> Vec<File> {
    text.split('\0')
        .filter_map(|l| {
            let mut parts = l.trim_start_matches('\n').splitn(3, '\t');
            let (a, r, path) = (parts.next()?, parts.next()?, parts.next()?);
            Some(File {
                path: path.to_string(),
                added: a.parse().ok(),
                removed: r.parse().ok(),
                untracked: false,
            })
        })
        .collect()
}

/// 作業フォルダがリポジトリの中のサブフォルダでも、パスはリポジトリの一番上から数える
/// （一覧と差分の取得で基準をそろえる）
fn top(dir: &str) -> Result<String> {
    Ok(git_ok(dir, &["rev-parse", "--show-toplevel"])?
        .trim()
        .to_string())
}

/// 比べる相手: HEAD。まだ 1 つも commit が無いリポジトリは HEAD が無いので、空の木
fn base(dir: &str) -> Result<&'static str> {
    let head = git(dir, &["rev-parse", "--verify", "--quiet", "HEAD"])?;
    Ok(if head.status.success() {
        "HEAD"
    } else {
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
    })
}

/// 何と比べた変更か
#[derive(Debug, Clone, PartialEq)]
pub enum Against {
    /// まだ commit していない変更（HEAD との差）
    Working,
    /// そのコミットで変わったもの
    Commit(String),
    /// このブランチ全体: 比べる相手のブランチから分かれたところ（merge-base）と、いまの作業コピー（commit していない分も）の差
    Base(String),
}

impl Against {
    /// 画面の問い合わせから。commit を優先する
    pub fn from_query(commit: Option<&str>, base: Option<&str>) -> Against {
        match (commit, base) {
            (Some(h), _) => Against::Commit(h.to_string()),
            (None, Some(b)) => Against::Base(b.to_string()),
            (None, None) => Against::Working,
        }
    }
}

/// 変わったファイルの一覧
pub fn files(dir: &str, against: &Against) -> Result<Vec<File>> {
    files_at(dir, against, None)
}

/// only を渡すと、そのパスだけを確かめる（差分 1 つのために、リポジトリ全体の一覧を作り直さない）
fn files_at(dir: &str, against: &Against, only: Option<&str>) -> Result<Vec<File>> {
    let dir = &top(dir)?;
    fn with_path<'a>(mut args: Vec<&'a str>, only: Option<&'a str>) -> Vec<&'a str> {
        if let Some(p) = only {
            args.extend(["--", p]);
        }
        args
    }
    let from = match against {
        Against::Commit(h) => {
            check_hash(h)?;
            return Ok(numstat(&git_ok(
                dir,
                &with_path(
                    vec!["show", "--numstat", "-z", "--no-renames", "--format=", h],
                    only,
                ),
            )?));
        }
        Against::Working => base(dir)?.to_string(),
        Against::Base(b) => merge_base(dir, b)?,
    };
    let mut files = numstat(&git_ok(
        dir,
        &with_path(vec!["diff", "--numstat", "-z", "--no-renames", &from], only),
    )?);
    let others = git_ok(
        dir,
        &with_path(
            vec!["ls-files", "-z", "--others", "--exclude-standard"],
            only,
        ),
    )?;
    for path in others.split('\0').filter(|p| !p.is_empty()) {
        files.push(File {
            path: path.to_string(),
            added: None,
            removed: None,
            untracked: true,
        });
    }
    Ok(files)
}

/// 比べる相手のブランチ（`main`・`origin/main` など）と HEAD が分かれたところ
fn merge_base(dir: &str, branch: &str) -> Result<String> {
    if branch.starts_with('-') || branch.contains("..") || branch.is_empty() {
        bail!("not a branch name: {branch}");
    }
    let refs = format!("{branch}^{{commit}}");
    git_ok(dir, &["rev-parse", "--verify", "--quiet", &refs])
        .with_context(|| format!("no branch named {branch}"))?;
    Ok(git_ok(dir, &["merge-base", branch, "HEAD"])?
        .trim()
        .to_string())
}

fn check_hash(hash: &str) -> Result<()> {
    if !(4..=40).contains(&hash.len()) || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("not a commit hash: {hash}");
    }
    Ok(())
}

/// 一覧に出したファイルだけを受ける（任意のパスを読ませない）
fn listed(dir: &str, against: &Against, path: &str) -> Result<File> {
    let Some(file) = files_at(dir, against, Some(path))?
        .into_iter()
        .find(|f| f.path == path)
    else {
        bail!("{path} is not among the changed files");
    };
    Ok(file)
}

/// まだ git に入っていない新しいファイルで、これより大きいものは差分を作らない（ログやダンプを丸ごと読まない）
const MAX_UNTRACKED_BYTES: u64 = 4 * MAX_PATCH_BYTES as u64;

/// 1 ファイルの差分。2 つ目は上限で切ったか
pub fn patch(dir: &str, against: &Against, path: &str) -> Result<(String, bool)> {
    let dir = &top(dir)?;
    let file = listed(dir, against, path)?;
    let out = match against {
        Against::Commit(h) => git_ok(dir, &["show", "--no-renames", "--format=", h, "--", path])?,
        // 新しいファイルは空のファイルとの差で見せる（差があると git diff は 1 で終わる）
        _ if file.untracked => {
            let size = std::fs::metadata(std::path::Path::new(dir).join(path))
                .map(|m| m.len())
                .unwrap_or(0);
            if size > MAX_UNTRACKED_BYTES {
                return Ok((String::new(), true));
            }
            let o = git(dir, &["diff", "--no-index", "--", "/dev/null", path])?;
            String::from_utf8_lossy(&o.stdout).into_owned()
        }
        // 一覧と同じ相手（HEAD か空の木）と、いまの作業コピーを比べる
        Against::Working => git_ok(dir, &["diff", "--no-renames", base(dir)?, "--", path])?,
        Against::Base(b) => {
            let from = merge_base(dir, b)?;
            git_ok(dir, &["diff", "--no-renames", &from, "--", path])?
        }
    };
    if out.len() <= MAX_PATCH_BYTES {
        return Ok((out, false));
    }
    let mut cut = MAX_PATCH_BYTES;
    while !out.is_char_boundary(cut) {
        cut -= 1;
    }
    Ok((out[..cut].to_string(), true))
}

/// 1 枚の画像の上限
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// 「変更」タブで変わる前と後を並べて見せる画像（拡張子 → Content-Type）。
/// SVG は中にスクリプトを持てるので入れない
pub fn image_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Before,
    After,
}

/// 変わった画像の、変わる前か後の中身と Content-Type。その側に無い（足した・消した）ときは None。
/// 前はコミットの親か HEAD、後はコミットか作業コピー
pub fn image(
    dir: &str,
    against: &Against,
    path: &str,
    side: Side,
) -> Result<Option<(Vec<u8>, &'static str)>> {
    let Some(mime) = image_type(path) else {
        bail!("{path} is not an image");
    };
    let dir = &top(dir)?;
    let file = listed(dir, against, path)?;
    let bytes = match (against, side) {
        // 最初のコミットには親が無いので、前は無い
        (Against::Commit(h), Side::Before) => blob(dir, &format!("{h}^:{path}"))?,
        (Against::Commit(h), Side::After) => blob(dir, &format!("{h}:{path}"))?,
        (_, Side::Before) if file.untracked => None,
        // 一覧と同じ相手（HEAD か空の木。空の木には何も無い）
        (Against::Working, Side::Before) => blob(dir, &format!("{}:{path}", base(dir)?))?,
        (Against::Base(b), Side::Before) => blob(dir, &format!("{}:{path}", merge_base(dir, b)?))?,
        (_, Side::After) => worktree_file(dir, path)?,
    };
    match bytes {
        Some(b) if b.len() as u64 > MAX_IMAGE_BYTES => bail!("{path} is too large to show"),
        b => Ok(b.map(|b| (b, mime))),
    }
}

/// git に入っている中身。その版に無ければ None。大きすぎるものは読む前に断る
fn blob(dir: &str, spec: &str) -> Result<Option<Vec<u8>>> {
    let size = git(dir, &["cat-file", "-s", spec])?;
    if !size.status.success() {
        return Ok(None);
    }
    if String::from_utf8_lossy(&size.stdout)
        .trim()
        .parse::<u64>()?
        > MAX_IMAGE_BYTES
    {
        bail!("{spec} is too large to show");
    }
    Ok(Some(git(dir, &["cat-file", "blob", spec])?.stdout))
}

/// 作業コピーの中身。消していれば None。
/// シンボリックリンクを（途中のフォルダも含めて）たどった先がリポジトリの外なら断る
fn worktree_file(dir: &str, path: &str) -> Result<Option<Vec<u8>>> {
    let Ok(real) = std::path::Path::new(dir).join(path).canonicalize() else {
        return Ok(None);
    };
    if !real.starts_with(std::path::Path::new(dir).canonicalize()?) {
        bail!("{path} is outside the repository");
    }
    let meta = std::fs::metadata(&real)?;
    if !meta.is_file() {
        bail!("{path} is not a regular file");
    }
    if meta.len() > MAX_IMAGE_BYTES {
        bail!("{path} is too large to show");
    }
    Ok(Some(std::fs::read(&real)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn whole_branch_against_its_base() {
        let d = repo("diff-branch");
        let run = |args: &[&str]| assert!(git(&d, args).unwrap().status.success(), "{args:?}");
        let main = git_ok(&d, &["symbolic-ref", "--short", "HEAD"]).unwrap();
        let main = main.trim();
        run(&["checkout", "-q", "-b", "feature"]);
        std::fs::write(Path::new(&d).join("a.txt"), "one\nTWO\n").unwrap();
        run(&[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-qam",
            "y",
        ]);
        // commit していない変更と、新しいファイルもブランチの変更に入る
        std::fs::write(Path::new(&d).join("b.txt"), "new\n").unwrap();
        let against = Against::Base(main.to_string());
        let paths: Vec<_> = files(&d, &against)
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["a.txt", "b.txt"]);
        assert!(patch(&d, &against, "a.txt").unwrap().0.contains("+TWO"));
        assert!(patch(&d, &against, "b.txt").unwrap().0.contains("+new"));
        assert!(patch(&d, &against, "bin.dat").is_err()); // 変わっていない
        for bad in ["--output=/tmp/x", "no-such-branch", "a..b", ""] {
            assert!(files(&d, &Against::Base(bad.into())).is_err(), "{bad}");
        }
        assert_eq!(
            Against::from_query(Some("abc1"), Some("main")),
            Against::Commit("abc1".into())
        );
        // まだ git に入っていない大きなファイルは差分を作らず、切ったことだけ知らせる
        std::fs::write(
            Path::new(&d).join("huge.log"),
            vec![b'x'; MAX_UNTRACKED_BYTES as usize + 1],
        )
        .unwrap();
        assert_eq!(
            patch(&d, &Against::Working, "huge.log").unwrap(),
            (String::new(), true)
        );
        // パスの中の * などは文字のまま（ほかのファイルに当たらない）
        assert!(patch(&d, &Against::Working, "*.txt").is_err());
    }

    fn repo(name: &str) -> String {
        let dir = crate::db::temp_dir(name).join("r");
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().into_owned();
        let run = |args: &[&str]| assert!(git(&d, args).unwrap().status.success(), "{args:?}");
        run(&["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("bin.dat"), [0u8, 1, 2, 0]).unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-q",
            "-m",
            "x",
        ]);
        d
    }

    #[test]
    fn working_changes_and_patches() {
        let d = repo("diff-work");
        assert!(files(&d, &Against::Working).unwrap().is_empty());
        std::fs::write(Path::new(&d).join("a.txt"), "one\n2\nthree\n").unwrap();
        std::fs::write(Path::new(&d).join("bin.dat"), [9u8, 0, 9]).unwrap();
        std::fs::write(Path::new(&d).join("new.txt"), "hello\n").unwrap();
        let list = files(&d, &Against::Working).unwrap();
        let a = list.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!((a.added, a.removed, a.untracked), (Some(2), Some(1), false));
        let b = list.iter().find(|f| f.path == "bin.dat").unwrap();
        assert_eq!((b.added, b.removed), (None, None)); // バイナリ
        assert!(list.iter().any(|f| f.path == "new.txt" && f.untracked));

        let (p, cut) = patch(&d, &Against::Working, "a.txt").unwrap();
        assert!(p.contains("-two") && p.contains("+three") && !cut);
        let (p, _) = patch(&d, &Against::Working, "new.txt").unwrap();
        assert!(p.contains("+hello"));
        // 一覧に無いパス・リポジトリの外は読ませない
        assert!(patch(&d, &Against::Working, "../../etc/passwd").is_err());
        assert!(patch(&d, &Against::Working, "missing.txt").is_err());
    }

    #[test]
    fn commit_changes_and_limits() {
        let d = repo("diff-commit");
        let head = git_ok(&d, &["rev-parse", "HEAD"]).unwrap();
        let head = head.trim();
        let list = files(&d, &Against::Commit(head[..7].to_string())).unwrap();
        assert!(list.iter().any(|f| f.path == "a.txt" && f.added == Some(2)));
        let (p, _) = patch(&d, &Against::Commit(head.to_string()), "a.txt").unwrap();
        assert!(p.contains("+one"));
        assert!(files(&d, &Against::Commit("HEAD".to_string())).is_err()); // ハッシュでない
        assert!(files(&d, &Against::Commit("--all".to_string())).is_err());
        assert!(files(&d, &Against::Commit("abc".to_string())).is_err()); // 短すぎる
        // 大きな差分は切る
        let big = "x\n".repeat(MAX_PATCH_BYTES);
        std::fs::write(Path::new(&d).join("a.txt"), big).unwrap();
        let (p, cut) = patch(&d, &Against::Working, "a.txt").unwrap();
        assert!(cut && p.len() <= MAX_PATCH_BYTES);
        assert!(files("/nonexistent", &Against::Working).is_err());
    }

    #[test]
    fn subfolders_and_unusual_names() {
        let d = repo("diff-names");
        let root = Path::new(&d);
        let run = |args: &[&str]| assert!(git(&d, args).unwrap().status.success(), "{args:?}");
        std::fs::create_dir_all(root.join("web/src")).unwrap();
        std::fs::write(root.join("web/src/App.vue"), "x\n").unwrap();
        std::fs::write(root.join("日本語.md"), "あ\n").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=a",
            "commit",
            "-q",
            "-m",
            "y",
        ]);
        std::fs::write(root.join("web/src/App.vue"), "y\n").unwrap();
        std::fs::write(root.join("日本語.md"), "い\n").unwrap();
        std::fs::write(root.join("新しい.txt"), "new\n").unwrap();
        run(&["mv", "a.txt", "b.txt"]);
        // 作業フォルダがサブフォルダでも、パスはリポジトリの一番上から。差分も取れる
        let sub = root.join("web").to_string_lossy().into_owned();
        let files = files(&sub, &Against::Working).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"web/src/App.vue"));
        assert!(paths.contains(&"日本語.md") && paths.contains(&"新しい.txt")); // 引用・エスケープされない
        assert!(paths.contains(&"a.txt") && paths.contains(&"b.txt")); // 名前の変更は削除と追加
        assert!(
            patch(&sub, &Against::Working, "web/src/App.vue")
                .unwrap()
                .0
                .contains("+y")
        );
        assert!(
            patch(&sub, &Against::Working, "日本語.md")
                .unwrap()
                .0
                .contains("+い")
        );
        assert!(
            patch(&sub, &Against::Working, "新しい.txt")
                .unwrap()
                .0
                .contains("+new")
        );
        assert!(
            patch(&sub, &Against::Working, "a.txt")
                .unwrap()
                .0
                .contains("-one")
        );
    }

    #[test]
    fn repository_without_commits() {
        let dir = crate::db::temp_dir("diff-empty").join("r");
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().into_owned();
        assert!(git(&d, &["init", "-q"]).unwrap().status.success());
        std::fs::write(dir.join("f.txt"), "a\n").unwrap();
        let list = files(&d, &Against::Working).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].untracked);
        // stage したあとで書き換えても、一覧と差分はいまの作業コピーの内容
        assert!(git(&d, &["add", "f.txt"]).unwrap().status.success());
        std::fs::write(dir.join("f.txt"), "a\nb\n").unwrap();
        let f = &files(&d, &Against::Working).unwrap()[0];
        assert_eq!((f.added, f.untracked), (Some(2), false));
        assert!(
            patch(&d, &Against::Working, "f.txt")
                .unwrap()
                .0
                .contains("+b")
        );
    }

    #[test]
    fn images_before_and_after() {
        let d = repo("diff-image");
        let root = Path::new(&d);
        let run = |args: &[&str]| assert!(git(&d, args).unwrap().status.success(), "{args:?}");
        let commit = |msg: &str| {
            run(&["add", "-A"]);
            run(&[
                "-c",
                "user.email=a@b",
                "-c",
                "user.name=a",
                "commit",
                "-q",
                "-m",
                msg,
            ]);
            git_ok(&d, &["rev-parse", "HEAD"])
                .unwrap()
                .trim()
                .to_string()
        };
        std::fs::write(root.join("logo.PNG"), b"old").unwrap();
        let first = commit("add");
        std::fs::write(root.join("logo.PNG"), b"new").unwrap();
        let second = commit("change");
        let get = |c: Option<&str>, path: &str, side| {
            image(&d, &Against::from_query(c, None), path, side).unwrap()
        };
        // コミット: 前は親の版、後はそのコミットの版。足したコミットに前は無い
        assert_eq!(
            get(Some(&second), "logo.PNG", Side::Before),
            Some((b"old".to_vec(), "image/png"))
        );
        assert_eq!(
            get(Some(&second), "logo.PNG", Side::After).unwrap().0,
            b"new"
        );
        assert_eq!(get(Some(&first), "logo.PNG", Side::Before), None);

        // まだ commit していない変更: 前は HEAD、後は作業コピー。新しいファイルに前は無く、消したファイルに後は無い
        std::fs::write(root.join("logo.PNG"), b"newer").unwrap();
        std::fs::write(root.join("shot.jpg"), b"jpg").unwrap();
        assert_eq!(get(None, "logo.PNG", Side::Before).unwrap().0, b"new");
        assert_eq!(get(None, "logo.PNG", Side::After).unwrap().0, b"newer");
        assert_eq!(get(None, "shot.jpg", Side::Before), None);
        assert_eq!(
            get(None, "shot.jpg", Side::After),
            Some((b"jpg".to_vec(), "image/jpeg"))
        );
        std::fs::remove_file(root.join("logo.PNG")).unwrap();
        assert_eq!(get(None, "logo.PNG", Side::After), None);

        // 画像でない・一覧に無い・シンボリックリンク・大きすぎるものは断る
        assert!(image(&d, &Against::Working, "a.txt", Side::After).is_err());
        assert!(image(&d, &Against::Working, "missing.png", Side::After).is_err());
        std::os::unix::fs::symlink("/etc/hostname", root.join("link.gif")).unwrap();
        assert!(image(&d, &Against::Working, "link.gif", Side::After).is_err());
        std::fs::remove_file(root.join("link.gif")).unwrap();
        // 途中のフォルダをリポジトリの外へのリンクに替えても、外のファイルは返さない
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/p.png"), b"in").unwrap();
        commit("assets");
        let outside = crate::db::temp_dir("diff-image-outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("p.png"), b"secret").unwrap();
        std::fs::remove_dir_all(root.join("assets")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("assets")).unwrap();
        assert!(image(&d, &Against::Working, "assets/p.png", Side::After).is_err());
        std::fs::remove_file(root.join("assets")).unwrap();
        // 作業コピーのファイルを置き換えたフォルダ（画像の名前のフォルダ）は断る
        std::fs::create_dir_all(root.join("assets/p.png")).unwrap();
        assert!(image(&d, &Against::Working, "assets/p.png", Side::After).is_err());
        std::fs::remove_dir_all(root.join("assets")).unwrap();
        let big = vec![0u8; MAX_IMAGE_BYTES as usize + 1];
        std::fs::write(root.join("big.webp"), &big).unwrap();
        assert!(image(&d, &Against::Working, "big.webp", Side::After).is_err());
        let third = commit("big");
        assert!(image(&d, &Against::Commit(third.clone()), "big.webp", Side::After).is_err());
    }

    #[test]
    fn image_types() {
        assert_eq!(image_type("a/b.jpeg"), Some("image/jpeg"));
        assert_eq!(image_type("x.gif"), Some("image/gif"));
        assert_eq!(image_type("x.webp"), Some("image/webp"));
        assert_eq!(image_type("x.svg"), None);
        assert_eq!(image_type("Makefile"), None);
    }
}
