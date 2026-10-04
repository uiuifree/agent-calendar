//! 新しい版の確認と入れ替え。GitHub の Releases（uiuifree/agent-calendar）の最新版を見て、
//! 配布物（tar.gz）をチェックサム（sha256）で確かめてから、動いている本体と入れ替える。
//! 開発中のビルド（target/ の下）は入れ替えない
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const REPO: &str = "uiuifree/agent-calendar";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// GitHub の最新のリリース
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Latest {
    pub version: String,
    /// リリースのページ（何が変わったかを読める）
    pub page: String,
    /// このマシン向けの配布物とチェックサム。無ければ入れ替えられない（確認だけ）
    pub asset: Option<String>,
    pub checksum: Option<String>,
}

/// "v1.2.3" / "1.2.3" → (1, 2, 3)。読めなければ None
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

/// このマシン向けの配布物の名前（release.yml の target と同じ）
pub fn target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

/// GitHub API の releases/latest の答えから、版とこのマシン向けの配布物を取り出す
pub fn parse_release(v: &Value, target: Option<&str>) -> Option<Latest> {
    let tag = v["tag_name"].as_str()?;
    let version = tag.trim_start_matches('v').to_string();
    parse_version(&version)?;
    let asset_url = |name: &str| {
        v["assets"].as_array()?.iter().find_map(|a| {
            (a["name"] == name)
                .then(|| a["browser_download_url"].as_str().map(str::to_string))
                .flatten()
        })
    };
    let (asset, checksum) = match target {
        Some(t) => {
            let name = format!("agent-calendar-{t}.tar.gz");
            (asset_url(&name), asset_url(&format!("{name}.sha256")))
        }
        None => (None, None),
    };
    Some(Latest {
        version,
        page: v["html_url"].as_str().unwrap_or_default().to_string(),
        asset,
        checksum,
    })
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(format!("agent-calendar/{CURRENT}"))
        .timeout(std::time::Duration::from_secs(60))
        .build()?)
}

/// GitHub に最新のリリースを聞く
pub fn check() -> Result<Latest> {
    let res = client()?
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("cannot reach GitHub")?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("no release has been published yet");
    }
    if !res.status().is_success() {
        bail!("GitHub answered {}", res.status());
    }
    let v: Value = res.json().context("GitHub's answer is not JSON")?;
    parse_release(&v, target()).context("the latest release has no version")
}

/// 開発中のビルド（cargo の target/ の下の release / debug。テストの本体がある deps も含む）か。
/// 入れ替えると開発の作業を上書きするので触らない
pub fn dev_build(exe: &Path) -> bool {
    let parts: Vec<_> = exe.components().map(|c| c.as_os_str()).collect();
    parts
        .windows(2)
        .any(|w| w[0] == "target" && (w[1] == "release" || w[1] == "debug"))
}

/// チェックサムのファイル（`<hex>  <name>`）と中身が合うか
pub fn verify(bytes: &[u8], checksum_file: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let want = checksum_file
        .split_whitespace()
        .next()
        .context("the checksum file is empty")?
        .to_ascii_lowercase();
    let got: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if want != got {
        bail!("the download does not match its checksum");
    }
    Ok(())
}

/// 新しい本体を、動いている本体と同じフォルダに書いてから名前を付け替える
/// （同じファイルシステムの中の付け替えなので、途中で止まっても壊れた本体は残らない）
pub fn replace_binary(new_bin: &Path, exe: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let staged = exe.with_extension("new");
    std::fs::copy(new_bin, &staged)
        .with_context(|| format!("cannot write next to {}", exe.display()))?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&staged, exe)?;
    Ok(())
}

/// 配布物を取って確かめ、動いている本体と入れ替える。戻り値は入れ替えた本体
pub fn install(latest: &Latest, work: &Path) -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if dev_build(&exe) {
        bail!(
            "this is a development build ({}); update it with cargo",
            exe.display()
        );
    }
    let (Some(asset), Some(checksum), Some(t)) = (&latest.asset, &latest.checksum, target()) else {
        bail!("the release has no download for this machine");
    };
    let http = client()?;
    let bytes = http.get(asset).send()?.error_for_status()?.bytes()?;
    let sum = http.get(checksum).send()?.error_for_status()?.text()?;
    verify(&bytes, &sum)?;
    unpack_and_replace(&bytes, t, work, &exe)?;
    Ok(exe)
}

/// tar.gz を作業フォルダに開き、中の本体で入れ替える
fn unpack_and_replace(bytes: &[u8], target: &str, work: &Path, exe: &Path) -> Result<()> {
    std::fs::create_dir_all(work)?;
    let archive = work.join("update.tar.gz");
    std::fs::write(&archive, bytes)?;
    let out = std::process::Command::new("tar")
        .arg("xzf")
        .arg(&archive)
        .arg("-C")
        .arg(work)
        .output()
        .context("cannot run tar")?;
    if !out.status.success() {
        bail!(
            "cannot unpack: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let new_bin = work
        .join(format!("agent-calendar-{target}"))
        .join("agent-calendar");
    if !new_bin.is_file() {
        bail!("the download has no agent-calendar binary");
    }
    replace_binary(&new_bin, exe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0-beta.1"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("x"), None);
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v0.10.0", "0.9.0")); // 数字として比べる
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn reads_the_release() {
        let v = json!({
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/uiuifree/agent-calendar/releases/tag/v0.2.0",
            "assets": [
                { "name": "agent-calendar-x86_64-unknown-linux-gnu.tar.gz", "browser_download_url": "https://x/a.tar.gz" },
                { "name": "agent-calendar-x86_64-unknown-linux-gnu.tar.gz.sha256", "browser_download_url": "https://x/a.sha256" },
                { "name": "agent-calendar-aarch64-apple-darwin.tar.gz", "browser_download_url": "https://x/m.tar.gz" }
            ]
        });
        let l = parse_release(&v, Some("x86_64-unknown-linux-gnu")).unwrap();
        assert_eq!(l.version, "0.2.0");
        assert_eq!(l.asset.as_deref(), Some("https://x/a.tar.gz"));
        assert_eq!(l.checksum.as_deref(), Some("https://x/a.sha256"));
        // チェックサムの無い配布物・このマシン向けが無いときは、入れ替えられない（確認だけ）
        let m = parse_release(&v, Some("aarch64-apple-darwin")).unwrap();
        assert!(m.asset.is_some() && m.checksum.is_none());
        assert!(parse_release(&v, None).unwrap().asset.is_none());
        assert!(parse_release(&json!({ "tag_name": "nightly" }), None).is_none());
        assert!(parse_release(&json!({}), None).is_none());
        assert!(target().is_some()); // テストを流すマシン（Linux / macOS）は配布物がある
    }

    #[test]
    fn checksums() {
        // "abc" の sha256
        let sum = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  agent-calendar.tar.gz\n";
        assert!(verify(b"abc", sum).is_ok());
        assert!(verify(b"abd", sum).is_err());
        assert!(verify(b"abc", &sum.to_uppercase()).is_ok());
        assert!(verify(b"abc", "").is_err());
    }

    #[test]
    fn development_builds_are_left_alone() {
        assert!(dev_build(Path::new(
            "/home/u/src/agent-calendar/target/release/agent-calendar"
        )));
        assert!(dev_build(Path::new("/x/target/debug/agent-calendar")));
        assert!(dev_build(Path::new(
            "/x/target/release/deps/agent_calendar-1a2b"
        ))); // テストの本体
        assert!(!dev_build(Path::new("/home/u/.local/bin/agent-calendar")));
    }

    #[test]
    fn unpacks_and_replaces_the_binary() {
        let dir = crate::db::temp_dir("update-replace");
        let exe = dir.join("bin").join("agent-calendar");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"old").unwrap();
        // 配布物と同じ形の tar.gz を作る
        let pkg = dir.join("pkg");
        let inner = pkg.join("agent-calendar-x86_64-unknown-linux-gnu");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("agent-calendar"), b"new").unwrap();
        let tgz = dir.join("a.tar.gz");
        assert!(
            std::process::Command::new("tar")
                .arg("czf")
                .arg(&tgz)
                .arg("-C")
                .arg(&pkg)
                .arg("agent-calendar-x86_64-unknown-linux-gnu")
                .status()
                .unwrap()
                .success()
        );
        let bytes = std::fs::read(&tgz).unwrap();
        unpack_and_replace(&bytes, "x86_64-unknown-linux-gnu", &dir.join("work"), &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!exe.with_extension("new").exists());
        // 中身が違う形・壊れた配布物は断る（本体は変わらない）
        assert!(
            unpack_and_replace(&bytes, "aarch64-apple-darwin", &dir.join("work2"), &exe).is_err()
        );
        assert!(
            unpack_and_replace(
                b"not a tarball",
                "x86_64-unknown-linux-gnu",
                &dir.join("work3"),
                &exe
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
    }
}
