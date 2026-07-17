//! `wiremock_helpers` — shared mounts for OpenAI-compat-shaped responses.
//!
//! Each helper takes a `&MockServer` and registers a `Mock` returning a
//! canned body. Tests assemble them via `mount_*` calls in setup, then
//! drive the real provider against `server.uri()`.
//!
//! Style: helpers are intentionally narrow — one wire shape per helper.
//! Tests that need a mutated body should `Mock::given(...)` directly.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mount a 200 SSE response that emits `chunks` joined by `\n\n` with a
/// terminal `data: [DONE]\n\n`. Each chunk is a JSON envelope shaped
/// like an OpenAI streaming `chat.completion.chunk`.
///
/// The matcher is `POST /v1/chat/completions`.
pub async fn mount_openai_chat_stream(server: &MockServer, chunks: &[&str]) {
    let mut body = String::new();
    for c in chunks {
        body.push_str("data: ");
        body.push_str(c);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

/// Mount a single fixed-status response with an empty body. Useful for
/// 4xx classification tests.
pub async fn mount_status_only(server: &MockServer, status: u16) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

/// Mount a 429 with `retry-after: <seconds>` header.
pub async fn mount_rate_limited(server: &MockServer, retry_after_secs: u32) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", retry_after_secs.to_string().as_str()),
        )
        .mount(server)
        .await;
}
