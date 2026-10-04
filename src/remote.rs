//! 別のマシンの `agent-calendar share` から記録を取ってきて手元に写す。
//! 通信は HTTPS。相手の証明書は、接続用の文字列に入っている指紋（sha256）と一致するものだけ受ける
use crate::db;
use crate::share::{self, Entry};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Remote {
    /// 画面に出すマシン名。手元のディレクトリ名にもなる
    pub name: String,
    /// https://<ip>:<port>
    pub url: String,
    pub token: String,
    /// 相手の証明書（DER）の sha256
    pub fingerprint: String,
}

/// `https://10.0.0.2:23847/#token=…&sha256=…` を分ける
pub fn parse_connection(s: &str) -> Result<(String, String, String)> {
    let (base, frag) = s
        .trim()
        .split_once('#')
        .context("the connection string has no #token=…&sha256=…")?;
    if !base.starts_with("https://") {
        bail!("the connection string must start with https://");
    }
    let get = |k: &str| {
        frag.split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
            .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_string)
    };
    let (Some(token), Some(fp)) = (get("token"), get("sha256")) else {
        bail!("the connection string needs token= and sha256= (hex)");
    };
    Ok((
        base.trim_end_matches('/').to_string(),
        token,
        fp.to_lowercase(),
    ))
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && !n.starts_with('.')
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// 名前を省くと URL のホスト部分（IP）にする
fn default_name(url: &str) -> String {
    let host = url.trim_start_matches("https://");
    host.split(':').next().unwrap_or(host).to_string()
}

/// つないでみて、記録の一覧が取れたら保存する
pub fn add(conn: &Connection, connection: &str, name: Option<&str>) -> Result<Remote> {
    let (url, token, fingerprint) = parse_connection(connection)?;
    let name = name
        .map(str::to_string)
        .unwrap_or_else(|| default_name(&url));
    if !valid_name(&name) {
        bail!("invalid name: {name} (letters, digits, '.', '_', '-')");
    }
    let r = Remote {
        name,
        url,
        token,
        fingerprint,
    };
    let n = r.list_files()?.len();
    conn.execute(
        "INSERT OR REPLACE INTO remotes (name, url, token, fingerprint) VALUES (?1, ?2, ?3, ?4)",
        params![r.name, r.url, r.token, r.fingerprint],
    )?;
    println!("added {} ({} transcripts available)", r.name, n);
    Ok(r)
}

pub fn list(conn: &Connection) -> Result<Vec<Remote>> {
    let mut stmt =
        conn.prepare("SELECT name, url, token, fingerprint FROM remotes ORDER BY name")?;
    let v = stmt
        .query_map([], |r| {
            Ok(Remote {
                name: r.get(0)?,
                url: r.get(1)?,
                token: r.get(2)?,
                fingerprint: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// 取ってきた記録と集計は消さない（日誌には残す）。以後取りに行かなくなるだけ
pub fn remove(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn.execute("DELETE FROM remotes WHERE name = ?1", [name])? > 0)
}

/// 取り込みは同時に 1 つだけ（自動更新と「読み直す」が重なると、同じ差分を二重に足してしまう）
static PULLING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 登録した全部のマシンから取ってくる。1 台つながらなくても他は続ける
pub fn pull_all(conn: &Connection) -> Result<()> {
    let _one_at_a_time = PULLING.lock().unwrap_or_else(|e| e.into_inner());
    for r in list(conn)? {
        match r.pull() {
            Ok(n) => println!("[remote] {}: updated {n} transcripts", r.name),
            Err(e) => eprintln!("[remote] {}: failed: {e:#}", r.name),
        }
    }
    Ok(())
}

impl Remote {
    pub fn dir(&self) -> PathBuf {
        db::path()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
            .join("remotes")
            .join(&self.name)
    }

    pub fn claude_dir(&self) -> PathBuf {
        self.dir().join("claude")
    }

    pub fn codex_dir(&self) -> PathBuf {
        self.dir().join("codex")
    }

    fn client(&self) -> Result<reqwest::blocking::Client> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pinned {
                fingerprint: self.fingerprint.clone(),
                provider,
            }))
            .with_no_client_auth();
        Ok(reqwest::blocking::Client::builder()
            .tls_backend_preconfigured(tls)
            .timeout(Duration::from_secs(60))
            .build()?)
    }

    fn list_files(&self) -> Result<Vec<Entry>> {
        let res = self
            .client()?
            .get(format!("{}/files", self.url))
            .bearer_auth(&self.token)
            .send()
            .with_context(|| format!("cannot reach {}", self.url))?;
        if !res.status().is_success() {
            bail!("{} answered {}", self.url, res.status());
        }
        Ok(res.json()?)
    }

    /// 変わった記録だけ取る。後ろに足されただけなら足された分だけ、縮んだ・書き直されたなら丸ごと。
    /// 戻り値は取り直したファイルの数
    pub fn pull(&self) -> Result<usize> {
        self.pull_into(&self.dir())
    }

    fn pull_into(&self, dir: &Path) -> Result<usize> {
        let client = self.client()?;
        // 取り込んだ記録には依頼文がそのまま入るので、本人だけが入れるようにする
        private_dir(dir)?;
        let mut n = 0;
        for e in self.list_files()? {
            // 相手が返すパスも信用しない（claude/・codex/ の下の .jsonl だけ書く）
            let Some(local) = local_path(dir, &e.path) else {
                continue;
            };
            let meta = std::fs::metadata(&local).ok();
            let have = meta.as_ref().map_or(0, |m| m.len());
            // 大きさも更新時刻も同じなら変わっていない。時刻だけ違えば書き直されたので丸ごと取る
            if have == e.size && meta.as_ref().map(share::mtime_ms) == Some(e.mtime) {
                continue;
            }
            let have = if have == e.size { 0 } else { have };
            let (offset, prefix) = if have > 0 && have < e.size {
                (have, share::sha256_hex(&std::fs::read(&local)?))
            } else {
                (0, String::new())
            };
            let fetch = |offset: u64, prefix: &str| {
                client
                    .get(format!("{}/file", self.url))
                    .bearer_auth(&self.token)
                    .query(&[
                        ("path", e.path.as_str()),
                        ("offset", &offset.to_string()),
                        ("prefix", prefix),
                    ])
                    .send()
            };
            let mut res = fetch(offset, &prefix)?;
            let mut append = offset > 0;
            if res.status() == reqwest::StatusCode::CONFLICT {
                res = fetch(0, "")?;
                append = false;
            }
            if !res.status().is_success() {
                eprintln!(
                    "[remote] {}: {} answered {}",
                    self.name,
                    e.path,
                    res.status()
                );
                continue;
            }
            write(&local, &res.bytes()?, append, e.mtime)?;
            n += 1;
        }
        Ok(n)
    }
}

fn local_path(dir: &Path, path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    let mut comps = p.components();
    let first = comps.next()?;
    let ok_root = matches!(first, Component::Normal(c) if c == "claude" || c == "codex");
    let ok_rest = comps.all(|c| matches!(c, Component::Normal(_)));
    (ok_root && ok_rest && p.extension().is_some_and(|x| x == "jsonl")).then(|| dir.join(p))
}

fn private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// 書いたあと、更新時刻を相手と同じにする（次の取り込みで変わったかを比べるため）
fn write(local: &Path, bytes: &[u8], append: bool, mtime_ms: i64) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    if let Some(d) = local.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(d)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .mode(0o600)
        .open(local)
        .with_context(|| format!("cannot write {}", local.display()))?;
    f.write_all(bytes)?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(mtime_ms.max(0) as u64);
    f.set_modified(t)?;
    Ok(())
}

/// 相手の証明書が、登録した指紋と一致するときだけ受ける（自分で作った証明書なので、CA では確かめられない）
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if share::sha256_hex(end_entity) == self.fingerprint {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "certificate fingerprint does not match the paired one".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connection_strings() {
        let (u, t, f) = parse_connection("https://10.0.0.2:23847/#token=ab12&sha256=CD34").unwrap();
        assert_eq!(
            (u.as_str(), t.as_str(), f.as_str()),
            ("https://10.0.0.2:23847", "ab12", "cd34")
        );
        for bad in [
            "http://x/#token=a&sha256=b",
            "https://x/",
            "https://x/#token=a",
            "https://x/#token=zz&sha256=ab",
            "https://x/#token=&sha256=ab",
        ] {
            assert!(parse_connection(bad).is_err(), "{bad}");
        }
        assert_eq!(default_name("https://10.0.0.2:23847"), "10.0.0.2");
        assert!(
            valid_name("ai-node") && !valid_name("../x") && !valid_name("") && !valid_name("a b")
        );
    }

    #[test]
    fn local_paths_stay_inside() {
        let d = Path::new("/data/remotes/n");
        assert_eq!(
            local_path(d, "claude/-r-a/s.jsonl"),
            Some(d.join("claude/-r-a/s.jsonl"))
        );
        assert_eq!(
            local_path(d, "codex/2026/09/17/rollout-a.jsonl"),
            Some(d.join("codex/2026/09/17/rollout-a.jsonl"))
        );
        for bad in [
            "claude/../../x.jsonl",
            "/etc/x.jsonl",
            "other/x.jsonl",
            "claude/x.txt",
            "claude",
        ] {
            assert!(local_path(d, bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn writes_whole_or_appends() {
        let f = db::temp_dir("remote-write").join("a/b.jsonl");
        write(&f, b"one\n", false, 1000).unwrap();
        write(&f, b"two\n", true, 2000).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"one\ntwo\n");
        write(&f, b"new\n", false, 3000).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&f).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(share::mtime_ms(&meta), 3000);
        assert_eq!(std::fs::read(&f).unwrap(), b"new\n");
    }

    #[test]
    fn stores_and_removes_remotes() {
        let conn = db::open_at(&db::temp_dir("remotes").join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO remotes VALUES ('ai-node', 'https://10.0.0.2:23847', 't', 'f')",
            [],
        )
        .unwrap();
        let v = list(&conn).unwrap();
        assert_eq!(v[0].name, "ai-node");
        assert!(v[0].claude_dir().ends_with("remotes/ai-node/claude"));
        assert!(v[0].codex_dir().ends_with("remotes/ai-node/codex"));
        assert!(remove(&conn, "ai-node").unwrap());
        assert!(!remove(&conn, "ai-node").unwrap());
    }

    /// 本物の share をこのプロセスで立て、指紋が合えば取れ、合わなければ断られることを確かめる
    #[tokio::test(flavor = "multi_thread")]
    async fn pulls_over_pinned_tls() {
        let home = db::temp_dir("remote-e2e");
        std::fs::create_dir_all(home.join(".claude/projects/-r-a")).unwrap();
        let src = home.join(".claude/projects/-r-a/s1.jsonl");
        std::fs::write(&src, "line1\n").unwrap();
        let (url, id) = share::spawn_for_test(home.clone()).await;
        let r = Remote {
            name: "t".into(),
            url: url.clone(),
            token: id.token.clone(),
            fingerprint: id.fingerprint.clone(),
        };
        let mirror = home.join("mirror");
        let dest = mirror.join("claude/-r-a/s1.jsonl");
        let r2 = r.clone();
        assert_eq!(
            {
                let m = mirror.clone();
                tokio::task::spawn_blocking(move || r2.pull_into(&m))
            }
            .await
            .unwrap()
            .unwrap(),
            1
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"line1\n");
        // 足された分だけ取る
        std::fs::write(&src, "line1\nline2\n").unwrap();
        let r2 = r.clone();
        assert_eq!(
            {
                let m = mirror.clone();
                tokio::task::spawn_blocking(move || r2.pull_into(&m))
            }
            .await
            .unwrap()
            .unwrap(),
            1
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"line1\nline2\n");
        // 書き直されて長くなった（先頭が違う）ら丸ごと取り直す
        std::fs::write(&src, "LINE1\nline2\nline3\n").unwrap();
        let r2 = r.clone();
        assert_eq!(
            {
                let m = mirror.clone();
                tokio::task::spawn_blocking(move || r2.pull_into(&m))
            }
            .await
            .unwrap()
            .unwrap(),
            1
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"LINE1\nline2\nline3\n");
        // 縮んだら丸ごと
        std::fs::write(&src, "x\n").unwrap();
        let r2 = r.clone();
        assert_eq!(
            {
                let m = mirror.clone();
                tokio::task::spawn_blocking(move || r2.pull_into(&m))
            }
            .await
            .unwrap()
            .unwrap(),
            1
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"x\n");
        // 同じ大きさで書き直されたら（時刻が変わる）丸ごと取り直す
        std::fs::write(&src, "y\n").unwrap();
        let f = std::fs::File::options().write(true).open(&src).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        drop(f);
        let r2 = r.clone();
        assert_eq!(
            {
                let m = mirror.clone();
                tokio::task::spawn_blocking(move || r2.pull_into(&m))
            }
            .await
            .unwrap()
            .unwrap(),
            1
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"y\n");
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&mirror).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        // 変わっていなければ取らない
        let r2 = r.clone();
        assert_eq!(
            {
                let m = mirror.clone();
                tokio::task::spawn_blocking(move || r2.pull_into(&m))
            }
            .await
            .unwrap()
            .unwrap(),
            0
        );
        // 指紋が違えば断られる・トークンが違えば断られる
        let bad_fp = Remote {
            fingerprint: "00".repeat(32),
            ..r.clone()
        };
        assert!(
            {
                let m = mirror.clone();
                tokio::task::spawn_blocking(move || bad_fp.pull_into(&m))
            }
            .await
            .unwrap()
            .is_err()
        );
        let bad_tok = Remote {
            token: "wrong".into(),
            ..r.clone()
        };
        assert!(
            {
                let m = mirror.clone();
                tokio::task::spawn_blocking(move || bad_tok.pull_into(&m))
            }
            .await
            .unwrap()
            .is_err()
        );
        // つないで確かめてから保存する
        let conn = db::open_at(&home.join("t.db")).unwrap();
        let s = format!("{url}/#token={}&sha256={}", id.token, id.fingerprint);
        let added = tokio::task::block_in_place(|| add(&conn, &s, Some("box"))).unwrap();
        assert_eq!(added.name, "box");
        assert!(tokio::task::block_in_place(|| add(&conn, &s, Some("../x"))).is_err());
    }
}
