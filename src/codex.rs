//! Codex CLI のセッション記録（`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`）を読む。
//! Claude Code と同じ `scan::Parsed` に揃えて、日誌・集計・要約を共通にする
use crate::pricing::Tokens;
use crate::scan::{BUCKET_SECS, Kind, Parsed, parse_ts};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 年・月・日のディレクトリをたどって rollout-*.jsonl を集める
pub fn session_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if root.is_dir() {
        walk(root, &mut files)?;
    }
    Ok(files)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let p = e?.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "jsonl")
            && p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
        {
            out.push(p);
        }
    }
    Ok(())
}

/// サブエージェントのスレッド（親から spawn されたもの）と、依頼が 1 つも無いものは None。
/// サブエージェントの分はまだ親に足していない（README の「対応していないこと」）
pub fn parse(text: &str) -> Option<Parsed> {
    let mut p = Parsed {
        source: "codex",
        ..Default::default()
    };
    let mut model = String::new();
    // token_count は累計で来るので、前回との差をその時刻の分として数える
    let mut prev = Tokens::default();
    for (i, line) in text.lines().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(ts) = v["timestamp"].as_str().and_then(parse_ts) else {
            continue;
        };
        if p.first_ts == 0 {
            p.first_ts = ts;
        }
        p.last_ts = p.last_ts.max(ts);
        p.last_uuid = v["ordinal"].as_i64().unwrap_or(i as i64).to_string();
        let pl = &v["payload"];
        match v["type"].as_str().unwrap_or("") {
            "session_meta" => {
                if pl["source"]["subagent"].is_object() {
                    return None;
                }
                p.id = pl["id"].as_str().unwrap_or("").to_string();
                p.cwd = pl["cwd"].as_str().unwrap_or("").to_string();
                p.branch = pl["git"]["branch"].as_str().unwrap_or("").to_string();
            }
            "turn_context" => model = pl["model"].as_str().unwrap_or("").to_string(),
            "response_item" => {
                *p.buckets
                    .entry(ts / 1000 / BUCKET_SECS * BUCKET_SECS)
                    .or_default() += 1;
                if pl["type"] == "message"
                    && pl["role"] == "user"
                    && let Some(t) = prompt_text(pl)
                {
                    p.prompts.push((ts, t));
                }
            }
            "event_msg" if pl["type"] == "token_count" => {
                let total = tokens(&pl["info"]["total_token_usage"]);
                if total == Tokens::default() || model.is_empty() {
                    continue;
                }
                let delta = Tokens {
                    input: total.input - prev.input,
                    output: total.output - prev.output,
                    cache_read: total.cache_read - prev.cache_read,
                    cache_5m: 0,
                    cache_1h: 0,
                };
                prev = total;
                if delta != Tokens::default() {
                    let bucket = ts / 1000 / BUCKET_SECS * BUCKET_SECS;
                    p.usage
                        .entry((bucket, model.clone(), Kind::Main, false))
                        .or_default()
                        .add(&delta);
                }
            }
            _ => {}
        }
    }
    if p.id.is_empty() || p.prompts.is_empty() {
        return None;
    }
    Some(p)
}

/// Codex の input_tokens はキャッシュから読んだ分を含む（input + output == total で確認）。
/// Claude Code と揃えて、キャッシュ分を cache_read に分ける
fn tokens(u: &Value) -> Tokens {
    let n = |k: &str| u[k].as_i64().unwrap_or(0);
    Tokens {
        input: n("input_tokens") - n("cached_input_tokens"),
        output: n("output_tokens"),
        cache_read: n("cached_input_tokens"),
        cache_5m: 0,
        cache_1h: 0,
    }
}

/// Codex が依頼の前に自動で差し込むもの（Codex CLI 0.154.0 の記録で確認）。
/// `<` で始まるだけでは除かない（人が `<task>…` のように書くこともある）
const INJECTED: &[&str] = &[
    "<environment_context>",
    "<recommended_plugins>",
    "<INSTRUCTIONS>",
    "# AGENTS.md instructions",
];

/// 人が打った依頼。環境の説明や AGENTS.md の差し込みは除く
pub fn prompt_text(payload: &Value) -> Option<String> {
    let text = payload["content"]
        .as_array()?
        .iter()
        .filter(|c| c["type"] == "input_text")
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    if text.is_empty() || INJECTED.iter().any(|p| text.starts_with(p)) {
        return None;
    }
    Some(text.to_string())
}

/// 要約に渡す本文: 依頼と Codex の返答の文章だけ（ツールの入出力は渡さない）
pub fn digest_blocks(text: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let pl = &v["payload"];
        if v["type"] != "response_item" || pl["type"] != "message" {
            continue;
        }
        if pl["role"] == "user" {
            if let Some(t) = prompt_text(pl) {
                out.push((true, t));
            }
        } else if pl["role"] == "assistant" {
            for c in pl["content"].as_array().into_iter().flatten() {
                if let Some(t) = c["text"]
                    .as_str()
                    .filter(|t| c["type"] == "output_text" && !t.trim().is_empty())
                {
                    out.push((false, t.to_string()));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ts: &str, ordinal: i64, kind: &str, payload: Value) -> String {
        serde_json::json!({"timestamp": ts, "ordinal": ordinal, "type": kind, "payload": payload})
            .to_string()
    }

    fn sample() -> String {
        [
            line("2026-09-16T23:12:42.000Z", 0, "session_meta", serde_json::json!({"id": "c1", "cwd": "/r/a", "git": {"branch": "main"}})),
            line("2026-09-16T23:12:43.000Z", 1, "turn_context", serde_json::json!({"model": "gpt-x"})),
            line("2026-09-16T23:12:44.000Z", 2, "response_item", serde_json::json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<environment_context>x</environment_context>"}]})),
            line("2026-09-16T23:12:45.000Z", 3, "response_item", serde_json::json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "# AGENTS.md instructions\n..."}]})),
            line("2026-09-16T23:12:46.000Z", 4, "response_item", serde_json::json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "テストを直して"}]})),
            line("2026-09-16T23:13:00.000Z", 5, "event_msg", serde_json::json!({"type": "token_count", "info": {"total_token_usage": {"input_tokens": 17906, "cached_input_tokens": 12160, "output_tokens": 203, "total_tokens": 18109}}})),
            line("2026-09-16T23:13:01.000Z", 6, "event_msg", serde_json::json!({"type": "token_count", "info": null})),
            line("2026-09-16T23:25:00.000Z", 7, "response_item", serde_json::json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "直しました"}]})),
            line("2026-09-16T23:25:01.000Z", 8, "event_msg", serde_json::json!({"type": "token_count", "info": {"total_token_usage": {"input_tokens": 20000, "cached_input_tokens": 13000, "output_tokens": 300, "total_tokens": 20300}}})),
            line("2026-09-16T23:25:02.000Z", 9, "event_msg", serde_json::json!({"type": "token_count", "info": {"total_token_usage": {"input_tokens": 20000, "cached_input_tokens": 13000, "output_tokens": 300, "total_tokens": 20300}}})),
            "not json".to_string(),
        ]
        .join("\n")
    }

    #[test]
    fn parses_codex_session() {
        let p = parse(&sample()).unwrap();
        assert_eq!(
            (p.source, p.id.as_str(), p.cwd.as_str(), p.branch.as_str()),
            ("codex", "c1", "/r/a", "main")
        );
        assert_eq!(
            p.prompts
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>(),
            ["テストを直して"]
        );
        assert_eq!(p.last_uuid, "9");
        assert_eq!(p.buckets.values().sum::<i64>(), 4);
        // 累計の差分を枠ごとに。キャッシュ分は cache_read に分ける。同じ累計の繰り返しは数えない
        let mut all = Tokens::default();
        for t in p.usage.values() {
            all.add(t);
        }
        assert_eq!(
            all,
            Tokens {
                input: 7000,
                output: 300,
                cache_read: 13000,
                cache_5m: 0,
                cache_1h: 0
            }
        );
        assert_eq!(p.usage.len(), 2);
        assert!(
            p.usage
                .keys()
                .all(|(_, m, k, fast)| m == "gpt-x" && *k == Kind::Main && !fast)
        );
    }

    #[test]
    fn skips_subagents_and_empty() {
        let sub = line(
            "2026-09-16T23:12:42.000Z",
            0,
            "session_meta",
            serde_json::json!({"id": "s", "source": {"subagent": {"thread_spawn": {}}}}),
        );
        assert!(parse(&format!("{sub}\n{}", sample())).is_none());
        let only_meta = line(
            "2026-09-16T23:12:42.000Z",
            0,
            "session_meta",
            serde_json::json!({"id": "e"}),
        );
        assert!(parse(&only_meta).is_none());
        assert!(prompt_text(&serde_json::json!({"content": "x"})).is_none());
        let msg = |t: &str| serde_json::json!({"content": [{"type": "input_text", "text": t}]});
        assert!(prompt_text(&msg("<recommended_plugins>\n...")).is_none());
        assert!(prompt_text(&msg("<INSTRUCTIONS>...")).is_none());
        // 人がタグで書いた依頼は残す
        assert_eq!(
            prompt_text(&msg("<task>テストを直して</task>")).as_deref(),
            Some("<task>テストを直して</task>")
        );
    }

    #[test]
    fn digest_has_prompts_and_answers() {
        assert_eq!(
            digest_blocks(&sample()),
            [
                (true, "テストを直して".to_string()),
                (false, "直しました".to_string())
            ]
        );
    }

    #[test]
    fn finds_rollout_files() {
        let dir = crate::db::temp_dir("codex-files");
        let day = dir.join("2026/09/17");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-a.jsonl"), "").unwrap();
        std::fs::write(day.join("other.jsonl"), "").unwrap();
        std::fs::write(day.join("rollout-b.txt"), "").unwrap();
        let files = session_files(&dir).unwrap();
        assert_eq!(files, [day.join("rollout-a.jsonl")]);
        assert!(session_files(&dir.join("missing")).unwrap().is_empty());
    }
}
