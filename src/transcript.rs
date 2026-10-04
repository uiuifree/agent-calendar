//! 会話の履歴を画面に出すために読む。人の依頼・エージェントの返答・ツールの呼び出しと結果に分ける。
//! 自動で差し込まれる文（system-reminder など）やサブエージェントの報告は人の発言として出さない
use crate::{codex, scan};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

/// ツールの入力・結果はここまでで切る（巨大なファイルの中身などで画面が重くならないように）
const TOOL_CHARS: usize = 4000;

#[derive(Debug, Serialize, PartialEq)]
pub struct Item {
    /// user / assistant / tool_use / tool_result
    pub kind: &'static str,
    /// unix ms（無ければ 0）
    pub ts: i64,
    pub text: String,
    /// tool_use のツール名
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
}

fn item(kind: &'static str, ts: i64, text: String, name: String) -> Item {
    Item {
        kind,
        ts,
        text,
        name,
    }
}

pub fn clip(s: &str) -> String {
    if s.chars().count() <= TOOL_CHARS {
        return s.to_string();
    }
    format!("{}\n…", s.chars().take(TOOL_CHARS).collect::<String>())
}

/// 記録を読み、新しい側から数えて [end - limit, end) を返す。戻り値は (全件数, その範囲)
pub fn read(
    file: &str,
    source: &str,
    end: Option<usize>,
    limit: usize,
) -> Result<(usize, usize, Vec<Item>)> {
    let text = std::fs::read_to_string(file).with_context(|| format!("cannot read {file}"))?;
    let all = if source == "codex" {
        codex_items(&text)
    } else {
        claude_items(&text)
    };
    let total = all.len();
    let end = end.unwrap_or(total).min(total);
    let start = end.saturating_sub(limit);
    Ok((
        total,
        start,
        all.into_iter().skip(start).take(end - start).collect(),
    ))
}

fn ts_of(v: &Value) -> i64 {
    v["timestamp"]
        .as_str()
        .and_then(scan::parse_ts)
        .unwrap_or(0)
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| {
                p["text"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| (p["type"] == "image").then(|| "[image]".to_string()))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

fn claude_items(text: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let ts = ts_of(&v);
        match v["type"].as_str() {
            Some("user") => {
                if let Some(t) = scan::prompt_text(&v) {
                    let t = with_images(t, &v["message"]["content"], "image");
                    out.push(item("user", ts, t, String::new()));
                    continue;
                }
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    if b["type"] == "tool_result" {
                        out.push(item(
                            "tool_result",
                            ts,
                            clip(&tool_result_text(&b["content"])),
                            String::new(),
                        ));
                    }
                }
            }
            Some("assistant") => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => {
                            if let Some(t) = b["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                out.push(item("assistant", ts, t.to_string(), String::new()));
                            }
                        }
                        Some("tool_use") => out.push(item(
                            "tool_use",
                            ts,
                            clip(&tool_input(&b["input"])),
                            b["name"].as_str().unwrap_or("").to_string(),
                        )),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// 依頼に画像が付いていたら、枚数ぶん [image] を添える（画像そのものは画面に出さない）
fn with_images(text: String, content: &Value, kind: &str) -> String {
    let n = content
        .as_array()
        .map_or(0, |a| a.iter().filter(|b| b["type"] == kind).count());
    if n == 0 {
        return text;
    }
    format!("{text}\n{}", vec!["[image]"; n].join(" "))
}

/// ツールの入力の見せ方。よく使う項目（コマンド・ファイル・パターン）があればそれを先に
fn tool_input(input: &Value) -> String {
    for key in [
        "command",
        "file_path",
        "pattern",
        "url",
        "query",
        "description",
        "prompt",
    ] {
        if let Some(s) = input[key].as_str() {
            return s.to_string();
        }
    }
    serde_json::to_string_pretty(input).unwrap_or_default()
}

fn codex_items(text: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["type"] != "response_item" {
            continue;
        }
        let (ts, p) = (ts_of(&v), &v["payload"]);
        match p["type"].as_str() {
            Some("message") if p["role"] == "user" => {
                if let Some(t) = codex::prompt_text(p) {
                    let t = with_images(t, &p["content"], "input_image");
                    out.push(item("user", ts, t, String::new()));
                }
            }
            Some("message") if p["role"] == "assistant" => {
                for c in p["content"].as_array().into_iter().flatten() {
                    if let Some(t) = c["text"].as_str().filter(|t| !t.trim().is_empty()) {
                        out.push(item("assistant", ts, t.to_string(), String::new()));
                    }
                }
            }
            Some("function_call") | Some("custom_tool_call") => {
                let input = p["arguments"]
                    .as_str()
                    .or(p["input"].as_str())
                    .unwrap_or("");
                // arguments は JSON の文字列。読めればよく使う項目を先に出す
                let shown = serde_json::from_str::<Value>(input)
                    .map(|j| tool_input(&j))
                    .unwrap_or_else(|_| input.to_string());
                out.push(item(
                    "tool_use",
                    ts,
                    clip(&shown),
                    p["name"].as_str().unwrap_or("").to_string(),
                ));
            }
            Some("function_call_output") | Some("custom_tool_call_output") => {
                out.push(item(
                    "tool_result",
                    ts,
                    clip(&tool_result_text(&p["output"])),
                    String::new(),
                ));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, text: &str) -> String {
        let f = crate::db::temp_dir(name).join("t.jsonl");
        std::fs::write(&f, text).unwrap();
        f.to_string_lossy().into_owned()
    }

    #[test]
    fn claude_transcript() {
        let text = r#"{"type":"user","timestamp":"2026-10-01T00:00:00Z","message":{"content":"直して"}}
{"type":"user","message":{"content":"<system-reminder>x</system-reminder>"}}
{"type":"assistant","timestamp":"2026-10-01T00:00:01Z","message":{"content":[{"type":"thinking","thinking":"…"},{"type":"text","text":"見ます"},{"type":"tool_use","name":"Bash","input":{"command":"git status","description":"x"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"clean"},{"type":"image"}]}]}}
{"type":"user","origin":{"kind":"peer"},"message":{"content":"report"}}
{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"sub"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"old":1}}]}}"#;
        let (total, start, items) = read(&write("tr-claude", text), "claude", None, 200).unwrap();
        assert_eq!((total, start), (5, 0));
        let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            ["user", "assistant", "tool_use", "tool_result", "tool_use"]
        );
        assert_eq!(items[0].ts, scan::parse_ts("2026-10-01T00:00:00Z").unwrap());
        assert_eq!(
            (items[2].name.as_str(), items[2].text.as_str()),
            ("Bash", "git status")
        );
        assert_eq!(items[3].text, "clean\n[image]");
        assert!(items[4].text.contains("\"old\": 1"));
        // 新しい側から 2 件、その前の 2 件
        let (_, start, last2) = read(&write("tr-claude2", text), "claude", None, 2).unwrap();
        assert_eq!((start, last2[0].kind), (3, "tool_result"));
        let (_, start, prev) = read(&write("tr-claude3", text), "claude", Some(3), 2).unwrap();
        assert_eq!((start, prev[0].kind, prev.len()), (1, "assistant", 2));
        assert!(read("/nonexistent.jsonl", "claude", None, 1).is_err());
    }

    #[test]
    fn user_images_are_marked() {
        let claude = r#"{"type":"user","message":{"content":[{"type":"text","text":"これ見て"},{"type":"image"},{"type":"image"}]}}"#;
        let (_, _, items) = read(&write("tr-img-claude", claude), "claude", None, 200).unwrap();
        assert_eq!(items[0].text, "これ見て\n[image] [image]");
        let codex = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"これ見て"},{"type":"input_image"}]}}"#;
        let (_, _, items) = read(&write("tr-img-codex", codex), "codex", None, 200).unwrap();
        assert_eq!(items[0].text, "これ見て\n[image]");
    }

    #[test]
    fn codex_transcript() {
        let text = r#"{"type":"response_item","timestamp":"2026-10-01T00:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"テストを直して"}]}}
{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":\"cargo test\"}"}}
{"type":"response_item","payload":{"type":"function_call_output","output":"ok"}}
{"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch"}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"直しました"}]}}
{"type":"event_msg","payload":{"type":"token_count"}}"#;
        let (_, _, items) = read(&write("tr-codex", text), "codex", None, 200).unwrap();
        let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            ["user", "tool_use", "tool_result", "tool_use", "assistant"]
        );
        assert_eq!(items[1].text, "cargo test");
        assert_eq!(items[3].text, "*** Begin Patch");
    }

    #[test]
    fn clips_long_tool_output() {
        assert_eq!(
            clip(&"x".repeat(TOOL_CHARS + 10)).chars().count(),
            TOOL_CHARS + 2
        );
        assert_eq!(tool_result_text(&serde_json::json!(5)), "5");
    }
}
