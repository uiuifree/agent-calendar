//! 別のマシン（ai-node など）で動かす「記録を渡す口」。HTTPS（自分で作った証明書）＋トークン。
//! 受け取る側は証明書の指紋で相手を確かめる（`remote.rs`）。画面も要約も持たず、.jsonl を渡すだけ
use crate::{db, opt};
use anyhow::{Context, Result};
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// 10000 より上で、よく使われる番号（Webmin 10000・memcached 11211・MongoDB 27017 など）と
/// OS が一時的な通信に割り当てる範囲（Linux は 32768〜）を避けた番号
pub const DEFAULT_BIND: &str = "0.0.0.0:23847";

/// 渡す記録の場所（名前, $HOME からの場所）。受け取る側も同じ名前で置く
pub const ROOTS: &[(&str, &str)] = &[("claude", ".claude/projects"), ("codex", ".codex/sessions")];

/// 証明書・鍵・トークン。初回に作って保存し、以後は同じものを使う（つなぎ直しを要らなくする）
pub struct Identity {
    pub cert_pem: String,
    pub key_pem: String,
    pub token: String,
    pub fingerprint: String,
}

fn dir() -> PathBuf {
    db::path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join("share")
}

pub fn identity() -> Result<Identity> {
    identity_at(&dir())
}

fn identity_at(dir: &Path) -> Result<Identity> {
    let (cert, key, token) = (dir.join("cert.pem"), dir.join("key.pem"), dir.join("token"));
    if !cert.exists() || !key.exists() || !token.exists() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let c = rcgen::generate_simple_self_signed(vec!["agent-calendar".to_string()])?;
        write_private(&cert, &c.cert.pem())?;
        write_private(&key, &c.signing_key.serialize_pem())?;
        write_private(&token, &random_hex(32)?)?;
    }
    let cert_pem = std::fs::read_to_string(&cert)?;
    let der = rustls_pemfile_der(&cert_pem)?;
    Ok(Identity {
        fingerprint: sha256_hex(&der),
        cert_pem,
        key_pem: std::fs::read_to_string(&key)?,
        token: std::fs::read_to_string(&token)?.trim().to_string(),
    })
}

/// 鍵とトークンは本人だけが読めるように置く
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    std::io::Write::write_all(&mut f, text.as_bytes())?;
    Ok(())
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("no random source: {e}"))?;
    Ok(hex(&buf))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// PEM の証明書から DER を取り出す（指紋は DER のハッシュ）
fn rustls_pemfile_der(pem: &str) -> Result<Vec<u8>> {
    use rustls::pki_types::{CertificateDer, pem::PemObject};
    let der = CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|e| anyhow::anyhow!("invalid certificate: {e:?}"))?;
    Ok(der.to_vec())
}

/// 受け取る側に貼ってもらう 1 行。IP・ポート・トークン・証明書の指紋をまとめる
pub fn connection_string(host: &str, port: u16, id: &Identity) -> String {
    format!(
        "https://{host}:{port}/#token={}&sha256={}",
        id.token, id.fingerprint
    )
}

/// `agent-calendar share pair`: 接続用の 1 行を出す
pub fn pair(args: &[String]) -> Result<()> {
    let bind: SocketAddr = opt(args, "--bind")
        .unwrap_or_else(|| DEFAULT_BIND.into())
        .parse()?;
    let host = match opt(args, "--host") {
        Some(h) => h,
        None if bind.ip().is_unspecified() => {
            lan_ip().context("cannot find this machine's LAN IP; pass --host <ip>")?
        }
        None => bind.ip().to_string(),
    };
    let id = identity()?;
    println!(
        "Run this on the machine that shows the calendar (keep it secret, it contains the access token):\n"
    );
    println!(
        "  agent-calendar remote add '{}'",
        connection_string(&host, bind.port(), &id)
    );
    Ok(())
}

/// 外向きの経路に使われる IP（パケットは送らない）
fn lan_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

struct Shared {
    token: String,
    home: PathBuf,
}

/// `agent-calendar share [--bind 0.0.0.0:23847]`
pub async fn run(args: &[String]) -> Result<()> {
    let bind: SocketAddr = opt(args, "--bind")
        .unwrap_or_else(|| DEFAULT_BIND.into())
        .parse()?;
    let id = identity()?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        id.cert_pem.into_bytes(),
        id.key_pem.into_bytes(),
    )
    .await?;
    let state = Arc::new(Shared {
        token: id.token,
        home: db::home(),
    });
    let app = router(state);
    println!(
        "[share] https://{bind}/  (run `agent-calendar share pair` to get the connection string)"
    );
    axum_server::bind_rustls(bind, tls)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

fn router(state: Arc<Shared>) -> Router {
    Router::new()
        .route("/files", get(files))
        .route("/file", get(file))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            check_token,
        ))
        .with_state(state)
}

/// トークンは長さに依らず全部比べる（早く抜けると一致した長さが時間で分かる）
fn token_ok(given: &str, want: &str) -> bool {
    let (a, b) = (given.as_bytes(), want.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn check_token(State(s): State<Arc<Shared>>, req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if !token_ok(given, &s.token) {
        return (StatusCode::UNAUTHORIZED, "bad token").into_response();
    }
    next.run(req).await
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Entry {
    /// `claude/<…>.jsonl` / `codex/<…>.jsonl`
    pub path: String,
    pub size: u64,
    /// 更新時刻（unix ミリ秒）。同じ大きさで書き直されたものを見分けるのに使う
    #[serde(default)]
    pub mtime: i64,
}

async fn files(State(s): State<Arc<Shared>>) -> Response {
    let home = s.home.clone();
    match tokio::task::spawn_blocking(move || list(&home)).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn list(home: &Path) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for (name, rel) in ROOTS {
        let root = home.join(rel);
        if root.is_dir() {
            walk(&root, &root, name, &mut out)?;
        }
    }
    Ok(out)
}

fn walk(root: &Path, dir: &Path, name: &str, out: &mut Vec<Entry>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let ft = e.file_type()?;
        let p = e.path();
        // シンボリックリンクはたどらない（外に出られないように）
        if ft.is_dir() {
            walk(root, &p, name, out)?;
        } else if ft.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
            let rel = p.strip_prefix(root)?.to_string_lossy().into_owned();
            let meta = e.metadata()?;
            out.push(Entry {
                path: format!("{name}/{rel}"),
                size: meta.len(),
                mtime: mtime_ms(&meta),
            });
        }
    }
    Ok(())
}

pub fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
    #[serde(default)]
    offset: u64,
    /// 受け取る側が持っている先頭 offset バイトの sha256。違えば 409 を返し、丸ごと取り直させる
    #[serde(default)]
    prefix: String,
}

async fn file(State(s): State<Arc<Shared>>, Query(q): Query<FileQuery>) -> Response {
    let home = s.home.clone();
    match tokio::task::spawn_blocking(move || read_from(&home, &q)).await {
        Ok(Ok(bytes)) => bytes.into_response(),
        Ok(Err(ReadError::Mismatch)) => {
            (StatusCode::CONFLICT, "prefix changed; fetch from 0").into_response()
        }
        Ok(Err(ReadError::Bad(msg))) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Ok(Err(ReadError::Io(e))) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

enum ReadError {
    Mismatch,
    Bad(String),
    Io(std::io::Error),
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// `claude/…` `codex/…` の下の .jsonl だけ。`..`・絶対パス・リンクで外へ出るものは断る
pub fn resolve(home: &Path, path: &str) -> Option<PathBuf> {
    let (name, rest) = path.split_once('/')?;
    let (_, rel) = ROOTS.iter().find(|(n, _)| *n == name)?;
    let rest = Path::new(rest);
    if !rest.extension().is_some_and(|x| x == "jsonl")
        || !rest.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let root = home.join(rel).canonicalize().ok()?;
    let full = root.join(rest).canonicalize().ok()?;
    full.starts_with(&root).then_some(full)
}

fn read_from(home: &Path, q: &FileQuery) -> std::result::Result<Vec<u8>, ReadError> {
    let Some(full) = resolve(home, &q.path) else {
        return Err(ReadError::Bad(format!(
            "not a shared transcript: {}",
            q.path
        )));
    };
    let mut f = std::fs::File::open(full)?;
    let len = f.metadata()?.len();
    if q.offset > len {
        return Err(ReadError::Mismatch);
    }
    if q.offset > 0 {
        let mut head = vec![0u8; q.offset as usize];
        f.read_exact(&mut head)?;
        if sha256_hex(&head) != q.prefix {
            return Err(ReadError::Mismatch);
        }
    }
    f.seek(SeekFrom::Start(q.offset))?;
    let mut out = Vec::new();
    f.read_to_end(&mut out)?;
    Ok(out)
}

/// テスト用: 127.0.0.1 の空いているポートで本物の share を立てる。戻り値は (URL, 証明書・トークン)
#[cfg(test)]
pub async fn spawn_for_test(home: PathBuf) -> (String, Identity) {
    let id = identity_at(&home.join("share-id")).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        id.cert_pem.clone().into_bytes(),
        id.key_pem.clone().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let app = router(Arc::new(Shared {
        token: id.token.clone(),
        home,
    }));
    let server = axum_server::from_tcp_rustls(listener, tls).unwrap();
    tokio::spawn(server.serve(app.into_make_service()));
    (format!("https://{addr}"), id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn home(name: &str) -> PathBuf {
        let h = db::temp_dir(name);
        std::fs::create_dir_all(h.join(".claude/projects/-r-a/s1/subagents")).unwrap();
        std::fs::write(h.join(".claude/projects/-r-a/s1.jsonl"), "line1\nline2\n").unwrap();
        std::fs::write(
            h.join(".claude/projects/-r-a/s1/subagents/agent.jsonl"),
            "sub\n",
        )
        .unwrap();
        std::fs::write(h.join(".claude/projects/-r-a/notes.txt"), "x").unwrap();
        std::fs::write(h.join("secret.jsonl"), "no").unwrap();
        std::os::unix::fs::symlink(
            h.join("secret.jsonl"),
            h.join(".claude/projects/-r-a/link.jsonl"),
        )
        .unwrap();
        h
    }

    #[test]
    fn lists_only_jsonl_recursively() {
        let h = home("share-lists_only_jsonl_recursively");
        let mut v = list(&h).unwrap();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        let paths: Vec<_> = v.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "claude/-r-a/s1.jsonl",
                "claude/-r-a/s1/subagents/agent.jsonl"
            ]
        );
        assert_eq!(v[0].size, 12);
    }

    #[test]
    fn refuses_paths_outside() {
        let h = home("share-refuses_paths_outside");
        assert!(resolve(&h, "claude/-r-a/s1.jsonl").is_some());
        for bad in [
            "claude/../secret.jsonl",
            "claude//etc/passwd.jsonl",
            "other/x.jsonl",
            "claude/-r-a/notes.txt",
            "claude/-r-a/link.jsonl",
            "nojsonl",
        ] {
            assert!(resolve(&h, bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn reads_tail_only_when_prefix_matches() {
        let h = home("share-reads_tail_only_when_prefix_matches");
        let q = |offset: u64, prefix: &str| FileQuery {
            path: "claude/-r-a/s1.jsonl".into(),
            offset,
            prefix: prefix.into(),
        };
        assert_eq!(read_from(&h, &q(0, "")).ok().unwrap(), b"line1\nline2\n");
        assert_eq!(
            read_from(&h, &q(6, &sha256_hex(b"line1\n"))).ok().unwrap(),
            b"line2\n"
        );
        assert!(matches!(
            read_from(&h, &q(6, &sha256_hex(b"LINE1\n"))),
            Err(ReadError::Mismatch)
        ));
        assert!(matches!(
            read_from(&h, &q(99, "")),
            Err(ReadError::Mismatch)
        ));
        let bad = FileQuery {
            path: "x".into(),
            offset: 0,
            prefix: String::new(),
        };
        assert!(matches!(read_from(&h, &bad), Err(ReadError::Bad(_))));
    }

    #[test]
    fn identity_is_created_once_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = db::temp_dir("share-id");
        let a = identity_at(&d).unwrap();
        let b = identity_at(&d).unwrap();
        assert_eq!((a.token.len(), a.fingerprint.len()), (64, 64));
        assert_eq!((&a.token, &a.fingerprint), (&b.token, &b.fingerprint));
        assert_eq!(
            std::fs::metadata(d.join("key.pem"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let s = connection_string("10.0.0.2", 23847, &b);
        assert!(s.starts_with("https://10.0.0.2:23847/#token="));
    }

    #[test]
    fn token_compare() {
        assert!(token_ok("abc", "abc"));
        assert!(!token_ok("abd", "abc"));
        assert!(!token_ok("ab", "abc"));
        assert!(!token_ok("", "abc"));
    }

    #[tokio::test]
    async fn router_requires_token() {
        let h = home("share-router_requires_token");
        let app = router(Arc::new(Shared {
            token: "t".into(),
            home: h,
        }));
        let req = |auth: Option<&str>, uri: &str| {
            let mut b = axum::http::Request::builder().uri(uri);
            if let Some(a) = auth {
                b = b.header("authorization", a);
            }
            b.body(axum::body::Body::empty()).unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(req(None, "/files"))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(req(Some("Bearer x"), "/files"))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(req(Some("Bearer t"), "/files"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let r = app
            .clone()
            .oneshot(req(
                Some("Bearer t"),
                "/file?path=claude/-r-a/s1.jsonl&offset=6&prefix=00",
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::CONFLICT);
        let r = app
            .oneshot(req(Some("Bearer t"), "/file?path=claude/../../x.jsonl"))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    }
}
