use crate::tools::{Tool, ToolResult};
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;

fn http_client() -> &'static Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("vibectl/0.1")
            .build()
            .expect("failed to build http client")
    })
}

pub struct WebFetch;

impl Tool for WebFetch {
    fn def(&self) -> super::ToolDef {
        super::ToolDef::new(
            "web_fetch",
            "Fetch the text content of a URL (web page or raw text). Use to look up docs, references, or assets.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Full URL to fetch (https/http)"
                    }
                },
                "required": ["url"]
            }),
        )
    }

    fn run(&self, args: &serde_json::Value, _cwd: &Path) -> Result<ToolResult> {
        let url = args
            .get("url")
            .and_then(serde_json::Value::as_str)
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
            .with_context(|| "web_fetch requires a 'url' starting with http(s)://")?
            .to_string();

        // The blocking-allowed network call runs on a dedicated OS thread with its own
        // mini runtime. Creating/dropping that runtime is safe there (we are never inside
        // a tokio async context on that thread), so we avoid the
        // "Cannot drop a runtime in a context where blocking is not allowed" panic.
        let fetch_url = url.clone();
        let text = std::thread::scope(|s| {
            s.spawn(move || -> Result<(String, String)> { fetch(&fetch_url) })
                .join()
                .map_err(|_| anyhow::anyhow!("web_fetch worker panicked"))?
        })?;

        let (content_type, body) = text;
        let is_html = content_type.contains("text/html") || body.trim_start().starts_with('<');
        let text = if is_html { strip_html(&body) } else { body };

        let mut content = format!("[fetched {url}]\n");
        content.push_str(text.trim());
        if content.chars().count() > 12000 {
            let cut: String = content.chars().take(12000).collect();
            content = format!("{cut}\n… (truncated at 12000 chars)");
        }
        Ok(ToolResult { content })
    }
}

fn fetch(url: &str) -> Result<(String, String)> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build worker runtime")?;
    rt.block_on(async move {
        let resp = http_client()
            .get(url)
            .send()
            .await
            .with_context(|| format!("failed to fetch {url}"))?
            .error_for_status()
            .with_context(|| format!("http error for {url}"))?;
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp
            .text()
            .await
            .with_context(|| format!("failed to read body of {url}"))?;
        Ok((content_type, body))
    })
}

fn strip_html(html: &str) -> String {
    let chars: Vec<char> = html.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0usize;
    let mut prev_ws = true;
    let mut skip: u8 = 0; // 0 = text, 1 = comment, 2 = script, 3 = style

    fn match_ci(chars: &[char], i: usize, needle: &str) -> bool {
        let (cnt, upper) = (chars.len().saturating_sub(i), needle.to_ascii_uppercase());
        chars[i..(i + cnt).min(i + upper.len())]
            .iter()
            .take(upper.len())
            .collect::<String>()
            .to_ascii_uppercase()
            == upper
    }

    while i < n {
        if skip == 1 {
            if match_ci(&chars, i, "-->") {
                i += 3;
                skip = 0;
            } else {
                i += 1;
            }
            continue;
        }
        if skip == 2 || skip == 3 {
            let close = if skip == 2 { "/script" } else { "/style" };
            if match_ci(&chars, i, close) {
                while i < n && chars[i] != '>' {
                    i += 1;
                }
                if i < n {
                    i += 1;
                }
                skip = 0;
            } else {
                i += 1;
            }
            continue;
        }

        if chars[i] == '<' {
            let start = i;
            i += 1;
            let mut tag = String::new();
            while i < n && chars[i] != '>' {
                tag.push(chars[i]);
                i += 1;
            }
            if i < n {
                i += 1;
            }
            let name = tag.trim().to_ascii_lowercase();
            if name.starts_with("!--") {
                skip = 1;
            } else if let Some(rest) = name.strip_prefix('/') {
                if (rest == "p" || rest == "div" || rest == "br" || rest == "li") && !prev_ws {
                    out.push(' ');
                    prev_ws = true;
                }
            } else if name == "script" {
                skip = 2;
            } else if name == "style" {
                skip = 3;
            } else {
                prev_ws = false;
            }
            let _ = start;
            continue;
        }

        let ch = chars[i];
        if ch == '\n' || ch == '\r' || ch == '\t' {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
            i += 1;
            continue;
        }
        if ch == ' ' {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
            i += 1;
            continue;
        }
        out.push(ch);
        prev_ws = false;
        i += 1;
    }
    out
}
