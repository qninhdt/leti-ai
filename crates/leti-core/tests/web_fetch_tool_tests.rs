//! Typed `WebFetchTool` boundary tests. The concrete HTTP/security behavior
//! lives in `leti-adapters`; these lock the injected seam, output mapping,
//! permission subject, defaults, and actionable error translation.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use leti_core::error::ToolError;
use leti_core::tools::builtins::web_fetch::{
    FetchError, FetchFormat, FetchRequest, FetchedPage, WebFetchInput, WebFetchTool, WebFetcher,
};
use leti_core::tools::{SchedulingMode, Tool};

use common::tool_ctx::minimal_tool_ctx;

struct StubFetcher {
    result: Result<FetchedPage, FetchError>,
}

#[async_trait]
impl WebFetcher for StubFetcher {
    async fn fetch(&self, _req: FetchRequest) -> Result<FetchedPage, FetchError> {
        self.result.clone()
    }
}

#[tokio::test]
async fn success_maps_fetched_page_to_tool_output() {
    let tool = WebFetchTool::with_fetcher(Arc::new(StubFetcher {
        result: Ok(FetchedPage {
            url_final: "https://example.com/final".into(),
            content: "# Title".into(),
            content_type: "text/html".into(),
            truncated: true,
            bytes: 123,
        }),
    }));

    let output = tool
        .run(
            minimal_tool_ctx(),
            WebFetchInput {
                url: "https://example.com/start".into(),
                format: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(output.url, "https://example.com/final");
    assert_eq!(output.content, "# Title");
    assert_eq!(output.content_type, "text/html");
    assert!(output.truncated);
}

#[tokio::test]
async fn blocked_host_maps_to_permission_denied() {
    let tool = WebFetchTool::with_fetcher(Arc::new(StubFetcher {
        result: Err(FetchError::BlockedHost("127.0.0.1".into())),
    }));

    let error = tool
        .run(
            minimal_tool_ctx(),
            WebFetchInput {
                url: "http://127.0.0.1".into(),
                format: Some(FetchFormat::Html),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(error, ToolError::PermissionDenied(_)));
}

#[tokio::test]
async fn unset_fetcher_returns_clear_configuration_error() {
    let error = WebFetchTool::default()
        .run(
            minimal_tool_ctx(),
            WebFetchInput {
                url: "https://example.com".into(),
                format: None,
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(error, ToolError::Io(message) if message.contains("not configured")));
}

#[test]
fn permission_subject_contains_the_full_url() {
    let input = WebFetchInput {
        url: "https://example.com/path?q=1".into(),
        format: None,
    };
    let request = WebFetchTool::default().permission(&input);
    assert_eq!(request.permission, "web_fetch:https://example.com/path?q=1");
    assert_eq!(
        WebFetchTool::default().concurrency(&input).mode,
        SchedulingMode::Concurrent
    );
}
