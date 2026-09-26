use crate::tools::ToolDef;
use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Role {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "tool")]
    Tool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(content.into()),
            tool_call_id: None,
            tool_calls: vec![],
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(content.into()),
            tool_call_id: None,
            tool_calls: vec![],
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.into()),
            tool_call_id: None,
            tool_calls: vec![],
            name: None,
        }
    }

    pub fn tool_result(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_call_id: Some(id.into()),
            tool_calls: vec![],
            name: None,
        }
    }

    #[allow(dead_code)]
    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: None,
            tool_call_id: None,
            tool_calls,
            name: None,
        }
    }

    /// Assistant message that has both text content AND tool calls.
    /// Some providers (OpenAI) allow this — the text is the "thinking" before calling tools.
    pub fn assistant_tool_calls_with_text(
        tool_calls: Vec<ToolCall>,
        text: impl Into<String>,
    ) -> Self {
        let t = text.into();
        Self {
            role: Role::Assistant,
            content: if t.is_empty() { None } else { Some(t) },
            tool_call_id: None,
            tool_calls,
            name: None,
        }
    }

    /// Return the text content of this message for token estimation purposes.
    /// For tool-call messages, concatenates all argument strings.
    pub fn content_text(&self) -> String {
        if let Some(c) = &self.content {
            return c.clone();
        }
        // assistant_tool_calls messages: estimate from arguments
        self.tool_calls
            .iter()
            .map(|tc| tc.arguments.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<ToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
}

impl Default for ChatRequest {
    fn default() -> Self {
        Self {
            model: String::new(),
            messages: vec![],
            temperature: 0.2,
            max_tokens: Some(4096),
            stream: true,
            tools: vec![],
            system: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolCallDelta {
    pub index: u32,
    pub id: String,
    pub name: String,
    pub args_delta: String,
}

#[derive(Debug, Clone)]
pub struct ChatChunk {
    pub content: Option<String>,
    pub tool_deltas: Vec<ToolCallDelta>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ChatResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: Usage,
}

pub type ChunkStream = futures::stream::BoxStream<'static, Result<ChatChunk>>;

#[async_trait]
pub trait Provider: Send + Sync {
    #[allow(dead_code)]
    fn name(&self) -> &str;
    async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse>;
    async fn chat_stream(&self, req: &ChatRequest) -> Result<ChunkStream>;
}

// ─── Retry policy ─────────────────────────────────────────────────────────────

/// Attempts per request, including the first.  Three is enough to ride out a
/// rate-limit window without stalling the agent for a minute.
pub const MAX_ATTEMPTS: u32 = 3;
const BASE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
const MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(8);

/// Is this status worth another attempt?
///
/// Retries 429 and 5xx, plus the two gateway-flavored codes that behave the
/// same way.  Deliberately *excludes* 400/401/403/404 — those are the caller's
/// fault and retrying only burns quota and delays the real error.
pub fn is_retryable(status: u16) -> bool {
    status == 408 || status == 425 || status == 429 || (500..600).contains(&status)
}

/// Honour a `Retry-After` header when the provider sends one, so we back off
/// exactly as long as asked instead of guessing.
pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(std::time::Duration::from_secs(
        secs.min(MAX_DELAY.as_secs()),
    ))
}

/// Exponential backoff for `attempt` (1-based), with jitter so concurrent
/// clients do not resynchronise into a thundering herd.
fn backoff(attempt: u32) -> std::time::Duration {
    let shift = attempt.saturating_sub(1).min(5);
    let scaled = BASE_DELAY.saturating_mul(1u32 << shift).min(MAX_DELAY);
    let jitter_span = scaled.as_millis() as u64 / 2;
    if jitter_span == 0 {
        return scaled;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter = nanos % (jitter_span + 1);
    scaled + std::time::Duration::from_millis(jitter)
}

/// POST a request, retrying transient upstream failures.
///
/// `build` is re-invoked per attempt because `send()` consumes the
/// `RequestBuilder`.  The response is returned as soon as it is successful,
/// non-retryable, or the attempt budget is spent — the caller then decides how
/// to render the error, which keeps the upstream body intact for diagnosis.
pub async fn send_with_retry<F>(build: F) -> Result<reqwest::Response>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt = 1u32;
    loop {
        let resp = build().send().await.map_err(|e| {
            anyhow::anyhow!("request failed (attempt {attempt}/{MAX_ATTEMPTS}): {e}")
        })?;

        let status = resp.status();
        if status.is_success() || attempt >= MAX_ATTEMPTS || !is_retryable(status.as_u16()) {
            return Ok(resp);
        }

        let wait = retry_after(resp.headers()).unwrap_or_else(|| backoff(attempt));
        drop(resp);
        tokio::time::sleep(wait).await;
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn retryable_covers_rate_limits_and_server_faults() {
        for status in [408, 425, 429, 500, 502, 503, 504, 529] {
            assert!(is_retryable(status), "{status} should be retryable");
        }
    }

    #[test]
    fn non_retryable_covers_client_faults() {
        // Retrying these is pure waste: the request will fail identically.
        for status in [400, 401, 403, 404, 422] {
            assert!(!is_retryable(status), "{status} should not be retryable");
        }
    }

    #[test]
    fn retry_after_parses_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "3".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(3)));
    }

    #[test]
    fn retry_after_clamps_absurd_values() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "86400".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(MAX_DELAY));
    }

    #[test]
    fn retry_after_tolerates_garbage_and_absence() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(retry_after(&headers), None);
        headers.insert(
            "retry-after",
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(
            retry_after(&headers),
            None,
            "HTTP-date form is not supported"
        );
    }

    #[test]
    fn backoff_grows_and_stays_capped() {
        assert!(backoff(1) < backoff(3));
        for attempt in 1..=10 {
            assert!(backoff(attempt) <= MAX_DELAY + MAX_DELAY / 2);
        }
    }

    // ── send_with_retry against a real socket ────────────────────────────────

    /// Minimal HTTP/1.1 server: replies with `script[i]` for the i-th connection
    /// and counts the connections it served.
    struct MockServer {
        addr: std::net::SocketAddr,
        hits: Arc<AtomicUsize>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl MockServer {
        async fn start(script: Vec<(u16, &'static str)>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let addr = listener.local_addr().expect("addr");
            let hits = Arc::new(AtomicUsize::new(0));
            let hits_for_task = Arc::clone(&hits);

            let handle = tokio::spawn(async move {
                for (status, retry_after) in script {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        return;
                    };
                    hits_for_task.fetch_add(1, Ordering::SeqCst);
                    // Drain the request head so the client sees a clean write.
                    let mut buf = [0u8; 2048];
                    let _ = socket.read(&mut buf).await;

                    let reason = match status {
                        200 => "OK",
                        401 => "Unauthorized",
                        429 => "Too Many Requests",
                        503 => "Service Unavailable",
                        _ => "Error",
                    };
                    let retry_header = match retry_after {
                        "" => String::new(),
                        secs => format!("Retry-After: {secs}\r\n"),
                    };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\n{retry_header}Content-Length: 2\r\n\r\n{{}}"
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                }
            });

            Self { addr, hits, handle }
        }

        fn url(&self) -> String {
            format!("http://{}/v1/chat/completions", self.addr)
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("client")
    }

    #[tokio::test]
    async fn succeeds_on_the_first_try_without_retrying() {
        let server = MockServer::start(vec![(200, "")]).await;
        let http = client();
        let url = server.url();
        let resp = send_with_retry(|| http.post(&url).body("{}"))
            .await
            .expect("request");
        assert!(resp.status().is_success());
        assert_eq!(server.hits(), 1);
    }

    #[tokio::test]
    async fn retries_a_rate_limit_then_succeeds() {
        let server = MockServer::start(vec![(429, ""), (200, "")]).await;
        let http = client();
        let url = server.url();
        let resp = send_with_retry(|| http.post(&url).body("{}"))
            .await
            .expect("request");
        assert!(
            resp.status().is_success(),
            "the 429 should have been ridden out"
        );
        assert_eq!(server.hits(), 2);
    }

    #[tokio::test]
    async fn retries_a_server_error_then_succeeds() {
        let server = MockServer::start(vec![(503, ""), (200, "")]).await;
        let http = client();
        let url = server.url();
        let resp = send_with_retry(|| http.post(&url).body("{}"))
            .await
            .expect("request");
        assert!(resp.status().is_success());
        assert_eq!(server.hits(), 2);
    }

    #[tokio::test]
    async fn gives_up_after_the_attempt_budget() {
        let script = vec![(503, ""); MAX_ATTEMPTS as usize];
        let server = MockServer::start(script).await;
        let http = client();
        let url = server.url();
        let resp = send_with_retry(|| http.post(&url).body("{}"))
            .await
            .expect("request");
        assert_eq!(resp.status().as_u16(), 503, "the final status is returned");
        assert_eq!(
            server.hits(),
            MAX_ATTEMPTS as usize,
            "no more than the budget"
        );
    }

    #[tokio::test]
    async fn does_not_retry_a_client_error() {
        // A bad key will never become good; retrying just triples the latency.
        let server = MockServer::start(vec![(401, "")]).await;
        let http = client();
        let url = server.url();
        let resp = send_with_retry(|| http.post(&url).body("{}"))
            .await
            .expect("request");
        assert_eq!(resp.status().as_u16(), 401);
        assert_eq!(server.hits(), 1, "401 must fail fast");
    }

    #[tokio::test]
    async fn honours_retry_after_instead_of_guessing() {
        // A 1s Retry-After must cost at least a second of wall clock.
        let server = MockServer::start(vec![(429, "1"), (200, "")]).await;
        let http = client();
        let url = server.url();
        let started = std::time::Instant::now();
        let resp = send_with_retry(|| http.post(&url).body("{}"))
            .await
            .expect("request");
        assert!(resp.status().is_success());
        assert!(
            started.elapsed() >= Duration::from_secs(1),
            "returned after {:?}, expected to wait for Retry-After",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn rebuilds_the_request_for_each_attempt() {
        // A RequestBuilder is consumed by send(); the closure must be re-runnable.
        let server = MockServer::start(vec![(500, ""), (500, ""), (200, "")]).await;
        let http = client();
        let url = server.url();
        let resp = send_with_retry(|| http.post(&url).body("{}"))
            .await
            .expect("request");
        assert!(resp.status().is_success());
        assert_eq!(server.hits(), 3);
    }
}
