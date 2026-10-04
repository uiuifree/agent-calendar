use crate::{
    db, diff, github, opt, pins, pricing, remote, repo, resume, scan, schedule, send, settings,
    stats, summarize, transcript,
};
use anyhow::{Context, Result};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::http::{Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{TimeZone, Timelike};
use rusqlite::{Connection, OptionalExtension};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 画面（`web/` を `npm run build` したもの）はバイナリに埋め込む。1 ファイルで配れるように
#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

/// 色を割り当てるリポジトリの数（画面のパレットが 8 色）。残りは「その他」
const COLOR_SLOTS: usize = 8;
/// 裏の処理が設定を見に行く間隔。実際に動かすかは設定の時間帯と間隔で決める
const TICK: Duration = Duration::from_secs(60);
const SUMMARIZE_PER_ROUND: usize = 10;

struct App {
    db: Mutex<Connection>,
    /// 要約の設定。--no-summarize で起動していれば None（画面の設定に出す。押しての要約もしない）
    summary: Option<SummarizeOpts>,
    /// 画面から指示を送って実行中のセッション
    running: send::Running,
    /// gh で取った組織ごとのリポジトリの一覧（取った時刻つき）。画面を切り替えるたびに GitHub へ行かないように
    gh_cache: Mutex<HashMap<String, (std::time::Instant, Vec<github::Remote>)>>,
}

/// GitHub の一覧を使い回す長さ（画面の「読み直す」で取り直せる）
const GH_CACHE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

type Shared = Arc<App>;

/// 裏で回す要約の設定。None なら要約しない
#[derive(Clone)]
struct SummarizeOpts {
    model: String,
    lang: summarize::Lang,
}

pub async fn run(args: &[String]) -> Result<()> {
    let port = opt(args, "--port").unwrap_or_else(|| "8082".into());
    let summarize = if args.iter().any(|a| a == "--no-summarize") {
        None
    } else {
        Some(SummarizeOpts {
            model: opt(args, "--summary-model").unwrap_or_else(|| summarize::DEFAULT_MODEL.into()),
            lang: summarize::Lang::from_arg(opt(args, "--summary-lang").as_deref())?,
        })
    };
    let summarizing = summarize.is_some();
    tokio::spawn(refresh_loop(summarize.clone()));

    let state = Arc::new(App {
        db: Mutex::new(db::open()?),
        summary: summarize,
        running: send::Running::default(),
        gh_cache: Mutex::new(HashMap::new()),
    });
    // 前回の停止で「実行中」のまま残った予定の実行を閉じてから、予定を動かし始める
    let closed = schedule::recover(&db::open()?)?;
    if closed > 0 {
        println!("[schedule] closed {closed} runs interrupted by the last stop");
    }
    tokio::spawn(schedule_loop(state.clone()));
    let api = Router::new()
        .route("/week", get(week))
        .route("/stats", get(stats_handler))
        .route("/session/{id}", get(session))
        .route("/session/{id}/resume", post(resume_handler))
        .route("/session/{id}/transcript", get(transcript_handler))
        .route("/session/{id}/summarize", post(summarize_handler))
        .route("/session/{id}/pin", post(pin_handler))
        .route("/permission", post(permission_handler))
        .route("/session/{id}/asks", get(asks_handler))
        .route("/session/{id}/changes", get(changes_handler))
        .route("/session/{id}/diff", get(diff_handler))
        .route("/pins", get(pins_handler))
        .route("/repo-sessions", get(repo_sessions_handler))
        .route(
            "/session/{id}/send",
            post(send_handler).layer(DefaultBodyLimit::max(SEND_BODY_LIMIT)),
        )
        .route("/repo-project", post(repo_project))
        .route("/rescan", post(rescan))
        .route("/settings", get(get_settings).post(put_settings))
        .route("/schedules", get(list_schedules).post(create_schedule))
        .route("/schedules/{id}", post(update_schedule))
        .route("/schedules/{id}/delete", post(delete_schedule))
        .route("/schedules/{id}/run", post(run_schedule))
        .route("/dirs", get(known_dirs))
        .route("/repos", get(repos_handler))
        .route("/repos/clone", post(clone_handler))
        .route(
            "/repos/start",
            post(start_handler).layer(DefaultBodyLimit::max(SEND_BODY_LIMIT)),
        )
        .with_state(state);
    let app = Router::new().nest("/api", api).fallback(asset).layer(
        axum::middleware::from_fn_with_state(Arc::new(port.clone()), check_host),
    );

    // 記録には社内の依頼文も入るので、外には出さない
    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("cannot listen on {addr}"))?;
    println!(
        "[serve] http://{addr}/  (summaries: {})",
        if summarizing { "on" } else { "off" }
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// SPA なので、知らないパスは index.html に戻す
async fn asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (file, body) = match Assets::get(path) {
        Some(f) if !path.is_empty() => (path, f),
        _ => match Assets::get("index.html") {
            Some(f) => ("index.html", f),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    "the web UI was not built into this binary",
                )
                    .into_response();
            }
        },
    };
    let mime = mime_guess::from_path(file).first_or_octet_stream();
    (
        [(header::CONTENT_TYPE, mime.as_ref().to_string())],
        body.data,
    )
        .into_response()
}

async fn refresh_loop(summarize: Option<SummarizeOpts>) {
    let (mut last_refresh, mut last_summary) = (None, None);
    loop {
        let opts = summarize.clone();
        let (lr, ls) = (last_refresh, last_summary);
        let r = tokio::task::spawn_blocking(move || -> Result<(bool, bool)> {
            let mut conn = db::open()?;
            let set = settings::load(&conn)?;
            let now = std::time::Instant::now();
            if !set.active_at(chrono::Local::now().hour()) {
                return Ok((false, false));
            }
            let refresh = settings::due(lr, set.refresh_minutes, now);
            if refresh {
                remote::pull_all(&conn)?;
                let n = scan::run(&mut conn)?;
                println!("[scan] re-read {n} sessions");
            }
            let summary = opts.is_some() && settings::due(ls, set.summary_minutes, now);
            if let Some(o) = opts.filter(|_| summary) {
                summarize::run(&conn, SUMMARIZE_PER_ROUND, &o.model, o.lang)?;
            }
            Ok((refresh, summary))
        })
        .await;
        let now = std::time::Instant::now();
        match r {
            Ok(Ok((refreshed, summarized))) => {
                if refreshed {
                    last_refresh = Some(now);
                }
                if summarized {
                    last_summary = Some(now);
                }
            }
            Ok(Err(e)) => eprintln!("[refresh] failed: {e:#}"),
            Err(e) => eprintln!("[refresh] crashed: {e}"),
        }
        tokio::time::sleep(TICK).await;
    }
}

#[derive(Debug)]
struct ApiError(StatusCode, anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, format!("{:#}", self.1)).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, e.into())
    }
}

/// Host が 127.0.0.1 / localhost の自分のポートでなければ断る。DNS rebinding（攻撃者のドメインを
/// 127.0.0.1 に向ける手口）だと Host も Origin も攻撃者のドメインになり、Origin の確認だけではすり抜ける
fn host_allowed(host: &str, port: &str) -> bool {
    ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|h| host == format!("{h}:{port}"))
}

async fn check_host(
    State(port): State<Arc<String>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !host_allowed(host, &port) {
        return (StatusCode::FORBIDDEN, format!("refused Host: {host}")).into_response();
    }
    next.run(req).await
}

/// 書き込みと起動の口は、この画面からの呼び出しだけを受ける。
/// 他のサイトのページから 127.0.0.1 へ投げられた POST は Origin が違うので断る
/// （JSON 本文を必須にしているので、フォーム送信のような単純なリクエストも通らない）
fn same_origin(headers: &HeaderMap) -> ApiResult<()> {
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    match headers.get("origin").and_then(|o| o.to_str().ok()) {
        None => Ok(()),
        Some(o) if o == format!("http://{host}") => Ok(()),
        Some(o) => Err(ApiError(
            StatusCode::FORBIDDEN,
            anyhow::anyhow!("refused cross-site request from {o}"),
        )),
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(Deserialize)]
struct Range {
    from: i64, // unix ms
    to: i64,
    /// 集計をホストで絞る。JSON の配列（手元は空文字）。無ければ全部
    #[serde(default)]
    machines: Option<String>,
}

#[derive(Serialize)]
struct WeekSession {
    id: String,
    repo: String,
    /// claude / codex
    source: String,
    /// 手元は空文字。別のマシンは `remote add` の名前
    machine: String,
    title: String,
    status: String,
    buckets: Vec<(i64, i64)>, // (unix 秒, 件数)
    /// (unix 秒, API 単価換算の USD)。日ごとの金額を出すのに使う
    usd: Vec<(i64, f64)>,
    /// 単価の分からないモデルを使っていた（金額が少なめに出ている）
    unpriced: bool,
}

#[derive(Serialize)]
struct RepoInfo {
    name: String,
    slot: Option<usize>,
    count: usize,
}

/// 範囲内に動きがあったセッションと、10 分ごとの件数。予定も添える（これから動く回と、まだ正常に終わっていない回）
async fn week(State(app): State<Shared>, Query(r): Query<Range>) -> ApiResult<Json<Value>> {
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut v = week_data(&conn, r.from / 1000, r.to / 1000)?;
    let now = chrono::Local::now();
    // これから動く回は今から先だけ（過ぎた回は実行の記録の側で出す）
    let from = chrono::Local
        .timestamp_millis_opt(r.from)
        .single()
        .filter(|f| *f > now)
        .unwrap_or(now);
    let mut plans = schedule::unfinished_runs(&conn, r.from, r.to)?;
    if let Some(to) = chrono::Local.timestamp_millis_opt(r.to).single() {
        plans.extend(schedule::upcoming(&conn, from, to)?);
    }
    v["plans"] = plans.into();
    Ok(Json(v))
}

/// from・to は unix 秒
fn week_data(conn: &Connection, from: i64, to: i64) -> Result<Value> {
    let mut sessions: Vec<WeekSession> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT a.session_id, a.bucket, a.n, s.repo, s.title, m.body,
                (SELECT text FROM prompts p WHERE p.session_id = s.id ORDER BY seq LIMIT 1), s.source, s.machine
         FROM activity a JOIN sessions s ON s.id = a.session_id
         LEFT JOIN summaries m ON m.session_id = s.id
         WHERE a.bucket >= ?1 AND a.bucket < ?2
         ORDER BY s.first_ts, a.bucket",
    )?;
    let mut rows = stmt.query([from, to])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let i = match index.get(&id) {
            Some(&i) => i,
            None => {
                let summary = row
                    .get::<_, Option<String>>(5)?
                    .and_then(|b| summarize::read(&b));
                let title = pins::title(summary.as_ref(), row.get(4)?, row.get(6)?);
                sessions.push(WeekSession {
                    id: id.clone(),
                    repo: row.get(3)?,
                    source: row.get(7)?,
                    machine: row.get(8)?,
                    title,
                    status: summary.map(|s| s.status).unwrap_or_default(),
                    buckets: Vec::new(),
                    usd: Vec::new(),
                    unpriced: false,
                });
                index.insert(id, sessions.len() - 1);
                sessions.len() - 1
            }
        };
        sessions[i].buckets.push((row.get(1)?, row.get(2)?));
    }

    let mut stmt = conn.prepare(
        "SELECT session_id, bucket, model, fast, SUM(input), SUM(output), SUM(cache_read), SUM(cache_5m), SUM(cache_1h)
         FROM usage WHERE bucket >= ?1 AND bucket < ?2 GROUP BY session_id, bucket, model, fast ORDER BY bucket",
    )?;
    let mut rows = stmt.query([from, to])?;
    while let Some(row) = rows.next()? {
        let Some(&i) = index.get(&row.get::<_, String>(0)?) else {
            continue;
        };
        let t = pricing::Tokens {
            input: row.get(4)?,
            output: row.get(5)?,
            cache_read: row.get(6)?,
            cache_5m: row.get(7)?,
            cache_1h: row.get(8)?,
        };
        let bucket: i64 = row.get(1)?;
        match pricing::cost(&row.get::<_, String>(2)?, row.get(3)?, &t) {
            Some(c) => match sessions[i].usd.last_mut() {
                Some((b, sum)) if *b == bucket => *sum += c,
                _ => sessions[i].usd.push((bucket, c)),
            },
            None => sessions[i].unpriced = true,
        }
    }

    let slots = color_slots(conn)?;
    let mut repos: HashMap<String, RepoInfo> = HashMap::new();
    for s in &sessions {
        repos
            .entry(s.repo.clone())
            .or_insert_with(|| RepoInfo {
                name: repo::name(&s.repo).to_string(),
                slot: slots.get(&s.repo).copied(),
                count: 0,
            })
            .count += 1;
    }
    // ホストの切り替えに出す一覧。手元と、登録した別のマシン（その週に記録が無くても出す）。
    // last_ts は各ホストの最後の記録（期間内に何も無いとき「最後は 8/5」と案内するため）
    let mut last: HashMap<String, i64> = HashMap::new();
    let mut stmt = conn.prepare("SELECT machine, MAX(last_ts) FROM sessions GROUP BY machine")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        last.insert(r.get(0)?, r.get(1)?);
    }
    let mut machines = vec![json!({ "id": "", "label": local_label(), "last_ts": last.get("") })];
    // 登録中のマシンと、登録を外しても記録が残っているマシン（外したあとも絞り込みを戻せるように）
    let mut names: Vec<String> = remote::list(conn)?.into_iter().map(|r| r.name).collect();
    names.extend(last.keys().filter(|k| !k.is_empty()).cloned());
    names.sort();
    names.dedup();
    for name in names {
        machines.push(json!({ "id": name, "label": name, "last_ts": last.get(&name) }));
    }
    Ok(json!({ "sessions": sessions, "repos": repos, "machines": machines }))
}

/// 手元のマシンの呼び名。WSL の中なら "WSL"、それ以外はホスト名
fn local_label() -> String {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    label_for(
        &release,
        &std::fs::read_to_string("/etc/hostname").unwrap_or_default(),
    )
}

fn label_for(osrelease: &str, hostname: &str) -> String {
    if osrelease.to_lowercase().contains("microsoft") {
        return "WSL".into();
    }
    Some(hostname.trim())
        .filter(|h| !h.is_empty())
        .unwrap_or("this PC")
        .to_string()
}

/// 全期間のセッション数の多い順に色を割り当てる。週をめくっても同じリポジトリは同じ色
fn color_slots(conn: &Connection) -> Result<HashMap<String, usize>> {
    let mut stmt = conn
        .prepare("SELECT repo FROM sessions GROUP BY repo ORDER BY COUNT(*) DESC, repo LIMIT ?1")?;
    let repos = stmt
        .query_map([COLOR_SLOTS as i64], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(repos.into_iter().enumerate().map(|(i, r)| (r, i)).collect())
}

async fn session(State(app): State<Shared>, Path(id): Path<String>) -> ApiResult<Response> {
    let detail = {
        let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        session_data(&conn, &id)?
    };
    let Some(mut detail) = detail else {
        return Ok((StatusCode::NOT_FOUND, "no such session").into_response());
    };
    let repo = detail["repo"].as_str().unwrap_or("").to_string();
    let cwd = detail["cwd"].as_str().unwrap_or("").to_string();
    let branch = detail["branch"].as_str().unwrap_or("").to_string();
    let (first, last) = (
        detail["first_ts"].as_i64().unwrap_or(0),
        detail["last_ts"].as_i64().unwrap_or(0),
    );
    let sid = id.clone();
    // 別のマシンのセッションは、手元の claude にも git にも聞けない
    let local = detail["machine"] == "";
    let is_claude = local && detail["source"] == "claude";
    let (commits, running, github) = tokio::task::spawn_blocking(move || {
        // 実行中かどうかは画面の表示に使うだけなので、claude が答えなければ「不明」で返す。
        // Codex には実行中の一覧を取る口が無い
        let running = if is_claude {
            resume::agents().ok().map(|list| {
                list.into_iter()
                    .find(|a| a.session_id == sid)
                    .map(|a| a.kind)
            })
        } else {
            None
        };
        let commits = if local {
            commits(&repo, first, last)
        } else {
            Vec::new()
        };
        // GitHub へのリンクは作業フォルダ（worktree ならそこ）の origin から
        let github = local
            .then(|| resume::workdir(&cwd, &repo).and_then(|d| github::links(d, &branch)))
            .flatten();
        (commits, running, github)
    })
    .await?;
    detail["commits"] = commits.into();
    detail["github"] = github.unwrap_or(Value::Null);
    detail["sending"] = app.running.contains(&id).into();
    detail["running"] = match running {
        Some(kind) => json!({ "known": true, "kind": kind }),
        None => json!({ "known": false }),
    };
    Ok(Json(detail).into_response())
}

/// コミット以外の詳細（コミットは git を叩くので DB の鍵の外で取る）
fn session_data(conn: &Connection, id: &str) -> Result<Option<Value>> {
    let Some(mut detail) = conn
        .query_row(
            "SELECT id, repo, cwd, branch, title, first_ts, last_ts, prompt_count, file, source, machine FROM sessions WHERE id = ?1",
            [id],
            |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?, "repo": r.get::<_, String>(1)?,
                    "cwd": r.get::<_, String>(2)?, "branch": r.get::<_, String>(3)?,
                    "title": r.get::<_, String>(4)?, "first_ts": r.get::<_, i64>(5)?,
                    "last_ts": r.get::<_, i64>(6)?, "prompt_count": r.get::<_, i64>(7)?,
                    "file": r.get::<_, String>(8)?, "source": r.get::<_, String>(9)?,
                    "machine": r.get::<_, String>(10)?,
                }))
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut stmt =
        conn.prepare("SELECT ts, text FROM prompts WHERE session_id = ?1 ORDER BY seq")?;
    let prompts = stmt
        .query_map([id], |r| {
            Ok(json!({ "ts": r.get::<_, i64>(0)?, "text": r.get::<_, String>(1)? }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let summary = conn
        .query_row(
            "SELECT body, model, cost_usd FROM summaries WHERE session_id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<f64>>(2)?,
                ))
            },
        )
        .optional()?
        .and_then(|(b, model, cost)| {
            let mut v = serde_json::to_value(summarize::read(&b)?).ok()?;
            v["model"] = model.into();
            v["cost_usd"] = cost.into();
            Some(v)
        });
    let cwd = detail["cwd"].as_str().unwrap_or("").to_string();
    let source = detail["source"].as_str().unwrap_or("").to_string();
    detail["prompts"] = prompts.into();
    detail["summary"] = summary.unwrap_or(Value::Null);
    detail["cost"] = stats::session_cost(conn, id)?;
    detail["resume_command"] = resume::resume_command(&source, &cwd, id).into();
    detail["pinned"] = pins::is_pinned(conn, id)?.into();
    Ok(Some(detail))
}

/// セッションの間に自分が入れたコミット。リポジトリが消えていれば空
fn commits(repo: &str, first_ms: i64, last_ms: i64) -> Vec<Value> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let email = git(&["config", "user.email"]).unwrap_or_default();
    let since = format!("@{}", first_ms / 1000);
    // 最後の発言のあとに手でコミットすることがあるので 10 分延ばす
    let until = format!("@{}", last_ms / 1000 + 600);
    let mut args = vec![
        "log",
        "--all",
        "--no-merges",
        "--since",
        &since,
        "--until",
        &until,
        "--format=%h%x1f%s%x1f%aI%x1f%D",
    ];
    let author = format!("--author={}", email.trim());
    if !email.trim().is_empty() {
        args.push(&author);
    }
    let Some(out) = git(&args) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\x1f').collect();
            let [hash, subject, date, refs] = f[..] else {
                return None;
            };
            Some(json!({ "hash": hash, "subject": subject, "date": date, "refs": refs }))
        })
        .collect()
}

async fn rescan(headers: HeaderMap, Json(_): Json<Value>) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let n = tokio::task::spawn_blocking(|| -> Result<usize> {
        let mut conn = db::open()?;
        remote::pull_all(&conn)?;
        scan::run(&mut conn)
    })
    .await??;
    Ok(Json(json!({ "rescanned": n })))
}

/// 予定の時刻が来たら起動する。自動更新の時間帯（設定）には左右されない（6 時の予定でも動く）
async fn schedule_loop(app: Shared) {
    loop {
        let due = tokio::task::spawn_blocking(|| -> Result<Vec<schedule::Due>> {
            schedule::take_due(&db::open()?, chrono::Local::now())
        })
        .await;
        match due {
            Ok(Ok(list)) => {
                for d in list {
                    println!("[schedule] {} ({})", d.name, d.id);
                    if let Err(e) = schedule::launch(&app.running, &d, rescan_after) {
                        eprintln!("[schedule] {} failed to start: {e:#}", d.name);
                    }
                }
            }
            Ok(Err(e)) => eprintln!("[schedule] failed: {e:#}"),
            Err(e) => eprintln!("[schedule] crashed: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

/// 実行が終わったら読み直して、追記された分を日誌に出す
fn rescan_after() {
    if let Ok(mut conn) = db::open() {
        let _ = scan::run(&mut conn);
    }
}

async fn list_schedules(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Json(json!({ "schedules": schedule::list(&conn)? })))
}

async fn create_schedule(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(i): Json<schedule::Input>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    let id = schedule::save(&conn, None, &i, chrono::Local::now())
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "id": id })))
}

async fn update_schedule(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(i): Json<schedule::Input>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    schedule::save(&conn, Some(id), &i, chrono::Local::now())
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "id": id })))
}

async fn delete_schedule(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(_): Json<Value>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    // 作業コピー（worktree）は消さない。中に残った変更を人が確かめてから消す
    Ok(Json(json!({ "deleted": schedule::delete(&conn, id)? })))
}

/// 今すぐ 1 回動かす（次の予定の時刻は変えない）
async fn run_schedule(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(_): Json<Value>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let due = {
        let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        schedule::get(&conn, id)?
    };
    let Some(d) = due else {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            anyhow::anyhow!("no such schedule"),
        ));
    };
    let run = schedule::launch(&app.running, &d, rescan_after)?;
    Ok(Json(json!({ "run_id": run })))
}

/// 予定の登録で選ぶディレクトリの候補（このマシンのセッションがあったもの、多い順）
async fn known_dirs(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut stmt = conn.prepare(
        "SELECT repo FROM sessions WHERE machine = '' AND repo != '' GROUP BY repo ORDER BY COUNT(*) DESC LIMIT 50",
    )?;
    let dirs = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|d| std::path::Path::new(d).is_dir())
        .collect::<Vec<_>>();
    Ok(Json(json!({ "dirs": dirs })))
}

async fn get_settings(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Json(
        json!({ "settings": settings::load(&conn)?, "summarizing": app.summary.is_some() }),
    ))
}

async fn put_settings(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(s): Json<settings::Settings>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    settings::save(&conn, &s).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "ok": true })))
}

async fn stats_handler(
    State(app): State<Shared>,
    Query(r): Query<Range>,
) -> ApiResult<Json<Value>> {
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Json(with_slots(
        &conn,
        stats::stats(
            &conn,
            r.from / 1000,
            r.to / 1000,
            parse_machines(r.machines.as_deref())?.as_deref(),
        )?,
    )?))
}

fn parse_machines(s: Option<&str>) -> ApiResult<Option<Vec<String>>> {
    s.map(serde_json::from_str::<Vec<String>>)
        .transpose()
        .map_err(|e| {
            ApiError(
                StatusCode::BAD_REQUEST,
                anyhow::anyhow!("machines must be a JSON array of strings: {e}"),
            )
        })
}

/// 集計の各リポジトリにも日誌と同じ色を付ける（月で見ると、その週に無いリポジトリも出るので）
fn with_slots(conn: &Connection, mut v: Value) -> Result<Value> {
    let slots = color_slots(conn)?;
    for p in v["projects"].as_array_mut().into_iter().flatten() {
        for r in p["repos"].as_array_mut().into_iter().flatten() {
            r["slot"] = r["repo"]
                .as_str()
                .and_then(|k| slots.get(k))
                .copied()
                .into();
        }
    }
    Ok(v)
}

#[derive(Deserialize)]
struct RepoProject {
    repo: String,
    project: String,
}

async fn repo_project(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(b): Json<RepoProject>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    stats::set_project(&conn, &b.repo, &b.project)?;
    Ok(Json(json!({ "ok": true })))
}

async fn resume_handler(
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(_): Json<Value>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    // claude の起動とつなぎ先の待ちで数秒かかるので、画面の読み取りとは別の接続で
    let r =
        tokio::task::spawn_blocking(move || -> Result<Value> { resume::resume(&db::open()?, &id) })
            .await?;
    r.map(Json).map_err(|e| ApiError(StatusCode::CONFLICT, e))
}

#[derive(Deserialize)]
struct Page {
    end: Option<usize>,
    limit: Option<usize>,
}

/// 会話の履歴。新しい側から limit 件（既定 200、最大 1000）。end より前をさかのぼって読める
async fn transcript_handler(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Query(p): Query<Page>,
) -> ApiResult<Response> {
    let row = {
        let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.query_row(
            "SELECT file, source FROM sessions WHERE id = ?1",
            [&id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    };
    let Some((file, source)) = row else {
        return Ok((StatusCode::NOT_FOUND, "no such session").into_response());
    };
    let limit = p.limit.unwrap_or(200).clamp(1, 1000);
    let (total, start, items) =
        tokio::task::spawn_blocking(move || transcript::read(&file, &source, p.end, limit))
            .await??;
    Ok(Json(json!({ "total": total, "start": start, "items": items })).into_response())
}

#[derive(Deserialize)]
struct SendBody {
    prompt: String,
    mode: send::Mode,
    /// 貼り付けた画像（無くてもよい）
    #[serde(default)]
    images: Vec<send::Image>,
}

/// 画像付きの指示の本文の上限（base64 で 4/3 倍になる分と、指示の文の分を見込む）
const SEND_BODY_LIMIT: usize = send::MAX_IMAGES * send::MAX_IMAGE_BYTES * 4 / 3 + 1024 * 1024;

/// Codex に渡す画像を一時的に置く場所（終わったら消す）
fn uploads_dir() -> std::path::PathBuf {
    db::path()
        .parent()
        .map(|p| p.join("uploads"))
        .unwrap_or_default()
}

/// 差分を見るフォルダ: このマシンのセッションの作業フォルダ（消えていればリポジトリ）
fn session_dir(app: &App, id: &str) -> ApiResult<String> {
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    let row = conn
        .query_row(
            "SELECT cwd, repo, machine FROM sessions WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((cwd, repo, machine)) = row else {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            anyhow::anyhow!("no such session"),
        ));
    };
    if !machine.is_empty() {
        return Err(ApiError(
            StatusCode::CONFLICT,
            anyhow::anyhow!("this session ran on {machine}; its files are not on this machine"),
        ));
    }
    resume::workdir(&cwd, &repo)
        .map(str::to_string)
        .ok_or_else(|| {
            ApiError(
                StatusCode::CONFLICT,
                anyhow::anyhow!("neither the working directory nor the repository exists"),
            )
        })
}

#[derive(Deserialize)]
struct ChangesQuery {
    /// あればそのコミットで変わったファイル。無ければまだ commit していない変更
    commit: Option<String>,
    /// 差分を見るファイル（/diff のとき）
    path: Option<String>,
}

/// 変わったファイルの一覧
async fn changes_handler(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<ChangesQuery>,
) -> ApiResult<Json<Value>> {
    let dir = session_dir(&app, &id)?;
    let d = dir.clone();
    let files = tokio::task::spawn_blocking(move || match q.commit.as_deref() {
        Some(h) => diff::commit_files(&d, h),
        None => diff::working(&d),
    })
    .await?
    .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "dir": dir, "files": files })))
}

/// 1 ファイルの差分
async fn diff_handler(
    State(app): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<ChangesQuery>,
) -> ApiResult<Json<Value>> {
    let dir = session_dir(&app, &id)?;
    let Some(path) = q.path else {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            anyhow::anyhow!("path is required"),
        ));
    };
    let (patch, truncated) =
        tokio::task::spawn_blocking(move || diff::patch(&dir, q.commit.as_deref(), &path))
            .await?
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "patch": patch, "truncated": truncated })))
}

#[derive(Deserialize)]
struct PermissionBody {
    request_id: String,
    allow: bool,
    /// このリポジトリでは今後も許可する（ルールを .claude/settings.local.json に足す）
    #[serde(default)]
    remember: bool,
}

/// 実行中の Claude からの許可の問い合わせに、画面で押した答えを返す
async fn permission_handler(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(b): Json<PermissionBody>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    app.running
        .answer(&b.request_id, b.allow, b.remember)
        .map_err(|e| ApiError(StatusCode::CONFLICT, e))?;
    Ok(Json(json!({ "ok": true })))
}

/// そのセッションで、画面の答えを待っている許可の問い合わせ（会話の画面を開き直したときに出し直す）
async fn asks_handler(State(app): State<Shared>, Path(id): Path<String>) -> Json<Value> {
    Json(json!({ "asks": app.running.asks(&id) }))
}

#[derive(Deserialize)]
struct PinBody {
    pinned: bool,
}

/// ピン留めを付ける・外す
async fn pin_handler(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<PinBody>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    pins::set(&conn, &id, b.pinned, chrono::Utc::now().timestamp_millis())?;
    Ok(Json(json!({ "pinned": b.pinned })))
}

#[derive(Deserialize)]
struct RepoQuery {
    repo: String,
}

/// リポジトリの画面で開く、そのリポジトリのセッションの一覧（新しい順に 100 件まで）
async fn repo_sessions_handler(
    State(app): State<Shared>,
    Query(q): Query<RepoQuery>,
) -> ApiResult<Json<Value>> {
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Json(
        json!({ "sessions": pins::in_repo(&conn, &q.repo, 100)? }),
    ))
}

async fn pins_handler(State(app): State<Shared>) -> ApiResult<Json<Value>> {
    let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Json(json!({ "pins": pins::list(&conn)? })))
}

/// 押したセッションを、いま要約する。記録を読み直してから作るので、直前の会話まで入る。
/// 要約は `claude -p` で時間がかかるので、画面の DB の口（app.db）は握らずに別に開く
async fn summarize_handler(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let Some(o) = app.summary.clone() else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            anyhow::anyhow!("summaries are off (started with --no-summarize)"),
        ));
    };
    // 同じセッションを要約中なら summarize::one が断る（裏の自動の要約とも共通の印）
    let s = tokio::task::spawn_blocking(move || {
        let mut conn = db::open()?;
        scan::run(&mut conn)?;
        summarize::one(&conn, &id, &o.model, o.lang)
    })
    .await??;
    Ok(Json(json!({ "summary": s })))
}

/// 続きの指示を送る。途中経過を NDJSON で流す。送った指示と返答は元のセッションの記録に追記される
async fn send_handler(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<SendBody>,
) -> ApiResult<Response> {
    same_origin(&headers)?;
    let refuse = |msg: &str| Err(ApiError(StatusCode::CONFLICT, anyhow::anyhow!("{msg}")));
    let prompt = b.prompt.trim().to_string();
    if prompt.is_empty() || prompt.chars().count() > send::MAX_PROMPT_CHARS {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            anyhow::anyhow!(
                "the instruction must be 1-{} characters",
                send::MAX_PROMPT_CHARS
            ),
        ));
    }
    let row = {
        let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        conn.query_row(
            "SELECT source, machine, cwd, repo FROM sessions WHERE id = ?1",
            [&id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
    };
    let Some((source, machine, cwd, repo)) = row else {
        return Ok((StatusCode::NOT_FOUND, "no such session").into_response());
    };
    if !machine.is_empty() {
        return refuse(&format!("this session ran on {machine}; continue it there"));
    }
    let Some(dir) = resume::workdir(&cwd, &repo).map(str::to_string) else {
        return refuse("neither the working directory nor the repository exists");
    };
    if source == "claude" {
        let sid = id.clone();
        let open = tokio::task::spawn_blocking(move || {
            resume::agents().map(|l| l.into_iter().any(|a| a.session_id == sid))
        })
        .await??;
        if open {
            return refuse(
                "this session is open in a terminal or in the background; continue it there",
            );
        }
    }
    let checked =
        send::check_images(&b.images).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    let (program, mut args) = send::command(&source, &id, b.mode);
    // 画面から動かすので、許可の問い合わせは画面で受ける（Claude だけ）
    let (stdin, cleanup) = send::attach(
        &source,
        &mut args,
        &prompt,
        (&b.images, &checked),
        &uploads_dir(),
        true,
    )?;
    // 終わったら読み直して、追記された分を日誌と履歴に出す
    let after = rescan_after;
    let job = send::Job {
        program: program.to_string(),
        args,
        source: source.clone(),
        id: id.clone(),
        cwd: dir,
        prompt: stdin,
        timeout: send::TIMEOUT,
        cleanup,
        ask: true,
    };
    let rx =
        send::start(&app.running, job, after).map_err(|e| ApiError(StatusCode::CONFLICT, e))?;
    ndjson(rx)
}

/// 途中経過を届いた順に 1 行ずつ流す
fn ndjson(rx: tokio::sync::mpsc::Receiver<String>) -> ApiResult<Response> {
    ndjson_after(None, rx)
}

/// first があれば、それを最初の 1 行として流してから途中経過を続ける
fn ndjson_after(
    first: Option<String>,
    rx: tokio::sync::mpsc::Receiver<String>,
) -> ApiResult<Response> {
    let stream = futures_util::stream::unfold((first, rx), |(first, mut rx)| async move {
        if let Some(line) = first {
            return Some((Ok::<_, std::convert::Infallible>(line), (None, rx)));
        }
        rx.recv().await.map(|line| (Ok(line), (None, rx)))
    });
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(axum::body::Body::from_stream(stream))
        .map_err(anyhow::Error::from)?)
}

#[derive(Deserialize)]
struct ReposQuery {
    /// true なら GitHub の一覧を取り直す
    #[serde(default)]
    refresh: bool,
}

/// リポジトリの画面: 設定で選んだ組織の GitHub のリポジトリと、手元の clone・作業の記録を並べる
async fn repos_handler(
    State(app): State<Shared>,
    Query(q): Query<ReposQuery>,
) -> ApiResult<Json<Value>> {
    let set = {
        let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        settings::load(&conn)?
    };
    let mut groups = Vec::new();
    for owner in &set.github_owners {
        let cached = app
            .gh_cache
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .get(owner)
            .filter(|(at, _)| !q.refresh && at.elapsed() < GH_CACHE)
            .map(|(_, l)| l.clone());
        let list = match cached {
            Some(l) => Ok(l),
            None => {
                let (s, o) = (set.clone(), owner.clone());
                let got = tokio::task::spawn_blocking(move || github::list(&s, &o)).await?;
                if let Ok(l) = &got {
                    app.gh_cache
                        .lock()
                        .map_err(|e| anyhow::anyhow!("{e}"))?
                        .insert(owner.clone(), (std::time::Instant::now(), l.clone()));
                }
                got
            }
        };
        groups.push((owner.clone(), list));
    }
    let roots = set.repo_roots.clone();
    let locals = tokio::task::spawn_blocking(move || github::locals(&roots)).await?;
    // 手元のリポジトリごとの、セッションの数と最後に動いた時刻
    let mut worked: HashMap<String, (i64, i64)> = HashMap::new();
    {
        let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut stmt = conn.prepare(
            "SELECT repo, COUNT(*), MAX(last_ts) FROM sessions WHERE machine = '' GROUP BY repo",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?;
        for row in rows {
            let (repo, n, last) = row?;
            worked.insert(repo, (n, last));
        }
    }
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|(owner, list)| match list {
            Ok(list) => {
                let repos: Vec<Value> = list
                    .into_iter()
                    .map(|r| {
                        let key = format!("{}/{}", r.owner, r.name).to_ascii_lowercase();
                        let local = locals
                            .iter()
                            .find(|l| l.slug == key)
                            .map(|l| l.path.clone());
                        let (sessions, last_ts) = local
                            .as_ref()
                            .and_then(|p| worked.get(p))
                            .map_or((0, None), |&(n, t)| (n, Some(t)));
                        let mut v = serde_json::to_value(&r).unwrap_or(Value::Null);
                        v["local"] = json!(local);
                        v["sessions"] = json!(sessions);
                        v["last_ts"] = json!(last_ts);
                        v
                    })
                    .collect();
                json!({ "owner": owner, "repos": repos })
            }
            Err(e) => json!({ "owner": owner, "error": format!("{e:#}"), "repos": [] }),
        })
        .collect();
    Ok(Json(json!({
        "owners": set.github_owners,
        "roots": set.repo_roots,
        "groups": groups,
    })))
}

#[derive(Deserialize)]
struct CloneBody {
    owner: String,
    name: String,
    root: String,
}

/// 手元に無いリポジトリを、探すフォルダのどれかへ clone する（設定で選んだ組織だけ）
async fn clone_handler(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(b): Json<CloneBody>,
) -> ApiResult<Json<Value>> {
    same_origin(&headers)?;
    let set = {
        let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        settings::load(&conn)?
    };
    let Some(claim) = app.running.claim(&format!(
        "clone:{}/{}",
        b.owner.to_ascii_lowercase(),
        b.name
    )) else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            anyhow::anyhow!("already cloning this repository"),
        ));
    };
    let path = tokio::task::spawn_blocking(move || {
        let _claim = claim;
        github::clone(&set, &b.owner, &b.name, &b.root)
    })
    .await?
    .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "path": path })))
}

#[derive(Deserialize)]
struct StartBody {
    dir: String,
    /// claude / codex
    agent: String,
    mode: send::Mode,
    prompt: String,
    #[serde(default)]
    images: Vec<send::Image>,
    /// 別の作業場所（git worktree）を新しいブランチで切ってそこで始める
    #[serde(default)]
    worktree: bool,
    /// そのブランチ名。空なら feature/日時
    #[serde(default)]
    branch: String,
}

/// 「ここで始める」で切るブランチの既定の名前（この PC の時刻）
fn default_branch(now: chrono::DateTime<chrono::Local>) -> String {
    format!("feature/{}", now.format("%Y%m%d-%H%M%S"))
}

/// リポジトリで新しいセッションを始める。途中経過は続きの指示と同じ形で流す（最初に session で ID が届く）
async fn start_handler(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(b): Json<StartBody>,
) -> ApiResult<Response> {
    same_origin(&headers)?;
    let bad = |msg: String| ApiError(StatusCode::BAD_REQUEST, anyhow::anyhow!("{msg}"));
    let prompt = b.prompt.trim().to_string();
    if prompt.is_empty() || prompt.chars().count() > send::MAX_PROMPT_CHARS {
        return Err(bad(format!(
            "the instruction must be 1-{} characters",
            send::MAX_PROMPT_CHARS
        )));
    }
    if b.agent != "claude" && b.agent != "codex" {
        return Err(bad("the agent must be claude or codex".into()));
    }
    let set = {
        let conn = app.db.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        settings::load(&conn)?
    };
    let dir = b.dir.clone();
    if !tokio::task::spawn_blocking(move || github::startable(&set, &dir)).await? {
        return Err(bad(format!(
            "{} is not a repository of the GitHub owners in settings",
            b.dir
        )));
    }
    let checked =
        send::check_images(&b.images).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
    // 作業場所を切るなら、先に作ってそこで始める。どこに切ったかを最初の 1 行で知らせる
    let (cwd, first) = if b.worktree {
        let branch = match b.branch.trim() {
            "" => default_branch(chrono::Local::now()),
            name => name.to_string(),
        };
        let (dir, br) = (b.dir.clone(), branch.clone());
        let path = tokio::task::spawn_blocking(move || {
            schedule::new_worktree(&dir, &br, &schedule::worktrees_dir())
        })
        .await?
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e))?;
        let line = json!({ "kind": "worktree", "path": path, "branch": branch });
        (path, Some(format!("{line}\n")))
    } else {
        (b.dir.clone(), None)
    };
    let (program, mut args) = send::command_new(&b.agent, b.mode);
    // 画面から動かすので、許可の問い合わせは画面で受ける（Claude だけ）
    let (stdin, cleanup) = send::attach(
        &b.agent,
        &mut args,
        &prompt,
        (&b.images, &checked),
        &uploads_dir(),
        true,
    )?;
    let job = send::Job {
        program: program.to_string(),
        args,
        source: b.agent.clone(),
        // 同じフォルダで二重に押しても 1 つだけ
        id: format!("new:{cwd}"),
        cwd,
        prompt: stdin,
        timeout: send::TIMEOUT,
        cleanup,
        ask: true,
    };
    let rx = send::start(&app.running, job, rescan_after)
        .map_err(|e| ApiError(StatusCode::CONFLICT, e))?;
    ndjson_after(first, rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn seed(conn: &Connection, id: &str, repo: &str, bucket: i64, title: &str) {
        conn.execute(
            "INSERT INTO sessions VALUES (?1, ?1, 0, 0, ?2, ?2, 'main', ?3, ?4, ?4, 'u', 1, 'claude', '')",
            params![id, repo, title, bucket * 1000],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO activity VALUES (?1, ?2, 3)",
            params![id, bucket],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO prompts VALUES (?1, 0, ?2, '最初の依頼')",
            params![id, bucket * 1000],
        )
        .unwrap();
    }

    #[test]
    fn week_picks_title_and_colors() {
        let conn = db::open_at(&db::temp_dir("week").join("t.db")).unwrap();
        for i in 0..9 {
            // repo0 が一番多く、repo8 は 9 番目なので色が付かない
            for j in 0..(10 - i) {
                seed(
                    &conn,
                    &format!("s{i}-{j}"),
                    &format!("/r/repo{i}"),
                    6000 + j as i64 * 600,
                    "",
                );
            }
        }
        conn.execute("UPDATE sessions SET title = 'AI題' WHERE id = 's0-1'", [])
            .unwrap();
        conn.execute(
            "INSERT INTO summaries VALUES ('s0-2', 'u', 'sonnet', '{\"title\":\"要約題\",\"bullets\":[],\"status\":\"完了\"}', 0, NULL)",
            [],
        )
        .unwrap();
        let w = week_data(&conn, 6000, 6600).unwrap();
        let s = w["sessions"].as_array().unwrap();
        let title = |id: &str| s.iter().find(|x| x["id"] == id).map(|x| x["title"].clone());
        assert_eq!(title("s0-0").unwrap(), "最初の依頼");
        assert_eq!(w["machines"][0]["id"], "");
        assert!(w["machines"][0]["last_ts"].is_i64());
        // 登録を外したマシンでも、記録があれば一覧に残す
        conn.execute(
            "UPDATE sessions SET machine = 'old-box' WHERE id = 's0-0'",
            [],
        )
        .unwrap();
        let w = week_data(&conn, 6000, 6600).unwrap();
        assert_eq!(w["machines"][1]["id"], "old-box");
        assert_eq!(w["repos"]["/r/repo0"]["slot"], 0);
        assert_eq!(w["repos"]["/r/repo8"]["slot"], Value::Null);
        assert_eq!(w["repos"]["/r/repo8"]["name"], "repo8");

        conn.execute("INSERT INTO usage VALUES ('s0-2', 7200, 'claude-opus-5-5', 'main', 0, 0, 1000000, 0, 0, 0)", []).unwrap();
        conn.execute("INSERT INTO usage VALUES ('s0-2', 7200, 'claude-sonnet-5-5', 'advisor', 0, 0, 1000000, 0, 0, 0)", []).unwrap();
        conn.execute(
            "INSERT INTO usage VALUES ('s0-2', 7800, 'unknown', 'main', 0, 0, 1, 0, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage VALUES ('ghost', 7200, 'claude-opus-5-5', 'main', 0, 0, 1, 0, 0, 0)",
            [],
        )
        .unwrap();
        let w = week_data(&conn, 6600, 8400).unwrap();
        let s = w["sessions"].as_array().unwrap();
        let get = |id: &str| s.iter().find(|x| x["id"] == id).unwrap().clone();
        assert_eq!(get("s0-1")["title"], "AI題");
        assert_eq!(get("s0-2")["title"], "要約題");
        // 初期の日本語の状態は符号にそろえて返す
        assert_eq!(get("s0-2")["status"], "done");
        assert_eq!(get("s0-2")["source"], "claude");
        assert_eq!(get("s0-2")["buckets"], json!([[7200, 3]]));
        assert_eq!(get("s0-2")["usd"], json!([[7200, 30.0]]));
        assert_eq!(get("s0-2")["unpriced"], true);
        assert_eq!(get("s0-1")["unpriced"], false);
    }

    #[test]
    fn session_detail() {
        let conn = db::open_at(&db::temp_dir("detail").join("t.db")).unwrap();
        seed(&conn, "a", "/r/x", 6000, "題");
        assert!(session_data(&conn, "missing").unwrap().is_none());
        let d = session_data(&conn, "a").unwrap().unwrap();
        assert_eq!(d["summary"], Value::Null);
        assert_eq!(d["prompts"][0]["text"], "最初の依頼");
        assert_eq!(d["cost"]["total"]["usd"], 0.0);
        assert_eq!(d["resume_command"], "cd /r/x && claude --resume a");
        conn.execute("INSERT INTO summaries VALUES ('a', 'u', 'sonnet', '{\"title\":\"t\",\"bullets\":[],\"status\":\"途中\"}', 0, 0.02)", []).unwrap();
        let d = session_data(&conn, "a").unwrap().unwrap();
        assert_eq!(d["summary"]["model"], "sonnet");
        assert_eq!(d["summary"]["cost_usd"], 0.02);
        assert_eq!(d["summary"]["status"], "wip");
        conn.execute("UPDATE sessions SET source = 'codex'", [])
            .unwrap();
        let d = session_data(&conn, "a").unwrap().unwrap();
        assert_eq!(d["resume_command"], "cd /r/x && codex resume a");
    }

    #[test]
    fn stats_rows_carry_colors() {
        let conn = db::open_at(&db::temp_dir("slots").join("t.db")).unwrap();
        seed(&conn, "a", "/r/x", 6000, "");
        let v = with_slots(&conn, stats::stats(&conn, 0, 100_000, None).unwrap()).unwrap();
        assert_eq!(v["projects"][0]["repos"][0]["slot"], 0);
    }

    #[tokio::test]
    async fn serves_embedded_assets() {
        let index = asset("/".parse().unwrap()).await;
        assert_eq!(index.status(), StatusCode::OK);
        assert!(
            index.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        // 知らないパスも画面に戻す（SPA）
        assert_eq!(
            asset("/journal/2026".parse().unwrap()).await.status(),
            StatusCode::OK
        );
    }

    #[test]
    fn machines_param() {
        assert_eq!(parse_machines(None).ok().unwrap(), None);
        assert_eq!(
            parse_machines(Some(r#"["","ai-node"]"#)).ok().unwrap(),
            Some(vec![String::new(), "ai-node".into()])
        );
        assert_eq!(
            parse_machines(Some("ai-node")).err().unwrap().0,
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn local_machine_label() {
        assert_eq!(label_for("6.6.87.2-microsoft-standard-WSL2", "box"), "WSL");
        assert_eq!(label_for("6.8.0-generic", "ai-node\n"), "ai-node");
        assert_eq!(label_for("6.8.0-generic", ""), "this PC");
    }

    #[test]
    fn host_check() {
        assert!(host_allowed("127.0.0.1:8082", "8082"));
        assert!(host_allowed("localhost:8082", "8082"));
        assert!(host_allowed("[::1]:8082", "8082"));
        assert!(!host_allowed("evil.example:8082", "8082"));
        assert!(!host_allowed("127.0.0.1:9999", "8082"));
        assert!(!host_allowed("", "8082"));
    }

    #[tokio::test]
    async fn host_middleware_blocks_rebinding() {
        use tower::ServiceExt;
        let app = Router::new().route("/", get(|| async { "ok" })).layer(
            axum::middleware::from_fn_with_state(Arc::new("8082".to_string()), check_host),
        );
        let req = |host: &str| {
            axum::http::Request::builder()
                .uri("/")
                .header("host", host)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(req("127.0.0.1:8082"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.oneshot(req("evil.example:8082"))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    /// テスト用の App。設定（組織 uiuifree、探すフォルダ root）を入れておく
    fn test_app(name: &str) -> (Shared, std::path::PathBuf) {
        let dir = db::temp_dir(name);
        let root = dir.join("root");
        let repo = root.join("agent-calendar/.git");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:uiuifree/agent-calendar.git\n",
        )
        .unwrap();
        let other = root.join("theirs/.git");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:someone/theirs.git\n",
        )
        .unwrap();
        let conn = db::open_at(&dir.join("t.db")).unwrap();
        settings::save(
            &conn,
            &settings::Settings {
                github_owners: vec!["uiuifree".into()],
                repo_roots: vec![root.to_string_lossy().into_owned()],
                ..Default::default()
            },
        )
        .unwrap();
        let app = Arc::new(App {
            db: Mutex::new(conn),
            summary: None,
            running: send::Running::default(),
            gh_cache: Mutex::new(HashMap::new()),
        });
        (app, root)
    }

    fn local_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("host", "127.0.0.1:8082".parse().unwrap());
        h
    }

    #[tokio::test]
    async fn repos_lists_cached_owners_with_local_clones() {
        let (app, root) = test_app("repos-list");
        let path = root.join("agent-calendar").to_string_lossy().into_owned();
        {
            let conn = app.db.lock().unwrap();
            seed(&conn, "s1", &path, 6000, "");
        }
        let remote = |name: &str| github::Remote {
            owner: "uiuifree".into(),
            name: name.into(),
            description: String::new(),
            private: true,
            archived: false,
            pushed_at: String::new(),
            url: String::new(),
        };
        app.gh_cache.lock().unwrap().insert(
            "uiuifree".into(),
            (
                std::time::Instant::now(),
                vec![remote("Agent-Calendar"), remote("not-cloned")],
            ),
        );
        let Json(v) = repos_handler(State(app.clone()), Query(ReposQuery { refresh: false }))
            .await
            .unwrap();
        let repos = &v["groups"][0]["repos"];
        assert_eq!(v["groups"][0]["owner"], "uiuifree");
        assert_eq!(repos[0]["local"], path);
        assert_eq!(repos[0]["sessions"], 1);
        assert_eq!(repos[0]["last_ts"], 6_000_000);
        assert_eq!(repos[1]["local"], Value::Null);
        assert_eq!(repos[1]["last_ts"], Value::Null);
    }

    #[tokio::test]
    async fn clone_and_start_refuse_outside_settings() {
        let (app, root) = test_app("repos-refuse");
        let r = root.to_string_lossy().into_owned();
        let clone = |owner: &str, name: &str, root: &str| CloneBody {
            owner: owner.into(),
            name: name.into(),
            root: root.into(),
        };
        // 設定に無い組織・置き場所・すでにあるフォルダは clone しない
        for b in [
            clone("someone", "x", &r),
            clone("uiuifree", "x", "/tmp"),
            clone("uiuifree", "agent-calendar", &r),
        ] {
            let e = clone_handler(State(app.clone()), local_headers(), Json(b))
                .await
                .unwrap_err();
            assert_eq!(e.0, StatusCode::BAD_REQUEST);
        }
        let start = |dir: String, agent: &str, prompt: &str| StartBody {
            dir,
            agent: agent.into(),
            mode: send::Mode::Read,
            prompt: prompt.into(),
            images: Vec::new(),
            worktree: false,
            branch: String::new(),
        };
        let ours = root.join("agent-calendar").to_string_lossy().into_owned();
        let theirs = root.join("theirs").to_string_lossy().into_owned();
        for b in [
            start(ours.clone(), "claude", "  "),
            start(ours.clone(), "gpt", "見て"),
            start(theirs, "claude", "見て"),
            start("/tmp".into(), "claude", "見て"),
        ] {
            let e = start_handler(State(app.clone()), local_headers(), Json(b))
                .await
                .unwrap_err();
            assert_eq!(e.0, StatusCode::BAD_REQUEST);
        }
        // 別のオリジンからは受けない
        let mut evil = local_headers();
        evil.insert("origin", "http://evil.example".parse().unwrap());
        assert!(
            start_handler(
                State(app.clone()),
                evil,
                Json(start(ours, "claude", "見て"))
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn pin_and_list() {
        let (app, _) = test_app("pin-api");
        seed(&app.db.lock().unwrap(), "s1", "/r/a", 6000, "題");
        let pin = |on: bool| {
            pin_handler(
                State(app.clone()),
                local_headers(),
                Path("s1".into()),
                Json(PinBody { pinned: on }),
            )
        };
        assert_eq!(pin(true).await.unwrap().0["pinned"], true);
        let Json(v) = pins_handler(State(app.clone())).await.unwrap();
        assert_eq!(v["pins"][0]["id"], "s1");
        assert_eq!(
            session_data(&app.db.lock().unwrap(), "s1")
                .unwrap()
                .unwrap()["pinned"],
            true
        );
        assert_eq!(pin(false).await.unwrap().0["pinned"], false);
        let Json(v) = pins_handler(State(app.clone())).await.unwrap();
        assert_eq!(v["pins"].as_array().unwrap().len(), 0);
        let Json(v) = repo_sessions_handler(
            State(app.clone()),
            Query(RepoQuery {
                repo: "/r/a".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(v["sessions"][0]["id"], "s1");
    }

    #[test]
    fn default_branch_names() {
        let at = chrono::Local
            .with_ymd_and_hms(2026, 10, 4, 15, 7, 9)
            .unwrap();
        assert_eq!(default_branch(at), "feature/20261004-150709");
    }

    #[tokio::test]
    async fn streams_the_first_line_before_events() {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send("b\n".to_string()).await.unwrap();
        drop(tx);
        let res = ndjson_after(Some("a\n".into()), rx).unwrap();
        let body = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"a\nb\n");
    }

    #[tokio::test]
    async fn permission_for_unknown_request_conflicts() {
        let (app, _) = test_app("permission-api");
        let body = PermissionBody {
            request_id: "nope".into(),
            allow: true,
            remember: false,
        };
        let e = permission_handler(State(app.clone()), local_headers(), Json(body))
            .await
            .unwrap_err();
        assert_eq!(e.0, StatusCode::CONFLICT);
        let Json(v) = asks_handler(State(app), Path("s1".into())).await;
        assert_eq!(v["asks"], json!([]));
    }

    #[tokio::test]
    async fn changes_and_diff_api() {
        let (app, root) = test_app("changes-api");
        let repo = root.join("work");
        std::fs::create_dir_all(&repo).unwrap();
        let r = repo.to_string_lossy().into_owned();
        assert!(
            Command::new("git")
                .args(["-C", &r, "init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(repo.join("n.txt"), "hi\n").unwrap();
        {
            let conn = app.db.lock().unwrap();
            seed(&conn, "s1", &r, 6000, "");
            seed(&conn, "far", &r, 6000, "");
            conn.execute(
                "UPDATE sessions SET machine = 'ai-node' WHERE id = 'far'",
                [],
            )
            .unwrap();
        }
        let q = |commit: Option<&str>, path: Option<&str>| {
            Query(ChangesQuery {
                commit: commit.map(str::to_string),
                path: path.map(str::to_string),
            })
        };
        let Json(v) = changes_handler(State(app.clone()), Path("s1".into()), q(None, None))
            .await
            .unwrap();
        assert_eq!(v["files"][0]["path"], "n.txt");
        let Json(v) = diff_handler(
            State(app.clone()),
            Path("s1".into()),
            q(None, Some("n.txt")),
        )
        .await
        .unwrap();
        assert!(v["patch"].as_str().unwrap().contains("+hi"));
        // パスが無い・コミットの指定がおかしい・別のマシン・無いセッション
        let status = |r: ApiResult<Json<Value>>| r.unwrap_err().0;
        assert_eq!(
            status(diff_handler(State(app.clone()), Path("s1".into()), q(None, None)).await),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(
                changes_handler(State(app.clone()), Path("s1".into()), q(Some("zz"), None)).await
            ),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(changes_handler(State(app.clone()), Path("far".into()), q(None, None)).await),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status(changes_handler(State(app.clone()), Path("none".into()), q(None, None)).await),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn summarize_needs_summaries_on() {
        let (app, _) = test_app("summarize-off");
        let e = summarize_handler(State(app), local_headers(), Path("s1".into()))
            .await
            .unwrap_err();
        assert_eq!(e.0, StatusCode::CONFLICT);
    }

    #[test]
    fn origin_check() {
        let h = |origin: Option<&str>| {
            let mut m = HeaderMap::new();
            m.insert("host", "127.0.0.1:8082".parse().unwrap());
            if let Some(o) = origin {
                m.insert("origin", o.parse().unwrap());
            }
            m
        };
        assert!(same_origin(&h(None)).is_ok());
        assert!(same_origin(&h(Some("http://127.0.0.1:8082"))).is_ok());
        let err = same_origin(&h(Some("https://evil.example"))).unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert_eq!(
            ApiError::from(anyhow::anyhow!("x"))
                .into_response()
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn commits_in_window_by_me() {
        let dir = db::temp_dir("commits");
        let repo = dir.to_string_lossy().into_owned();
        let git = |envs: &[(&str, &str)], args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .envs(envs.iter().copied())
                .status()
                .unwrap();
            assert!(ok.success());
        };
        git(&[], &["init", "-q"]);
        git(&[], &["config", "user.email", "me@example.com"]);
        git(&[], &["config", "user.name", "me"]);
        let at = |t: &'static str| [("GIT_AUTHOR_DATE", t), ("GIT_COMMITTER_DATE", t)];
        git(
            &at("2026-10-01T10:00:00Z"),
            &["commit", "-q", "--allow-empty", "-m", "前"],
        );
        git(
            &at("2026-10-01T12:00:00Z"),
            &["commit", "-q", "--allow-empty", "-m", "中"],
        );
        let other = [
            at("2026-10-01T12:05:00Z").as_slice(),
            &[("GIT_AUTHOR_EMAIL", "other@example.com")],
        ]
        .concat();
        git(&other, &["commit", "-q", "--allow-empty", "-m", "他人"]);
        let start = chrono::DateTime::parse_from_rfc3339("2026-10-01T11:00:00Z")
            .unwrap()
            .timestamp_millis();
        let c = commits(&repo, start, start + 3_600_000);
        assert_eq!(
            c.iter()
                .map(|c| c["subject"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["中"]
        );
        assert!(commits("/nonexistent", 0, 1).is_empty());
    }
}
