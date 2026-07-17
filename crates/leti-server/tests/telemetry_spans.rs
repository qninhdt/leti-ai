//! CP2 telemetry: the request-scoped correlation span is created with a
//! `request_id` field on every HTTP request.
//!
//! Uses a custom capturing `tracing` layer set as the thread-local default
//! and a current-thread runtime so the subscriber stays in scope across
//! awaits (a multi-thread runtime would lose the thread-local default).

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::util::ServiceExt;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{Layer, Registry};

mod support;

/// One captured span: its name + the field names present on creation.
#[derive(Default)]
struct Captured {
    spans: Vec<(String, Vec<String>)>,
}

#[derive(Clone)]
struct CaptureLayer {
    out: Arc<Mutex<Captured>>,
}

struct FieldNames(Vec<String>);

impl Visit for FieldNames {
    fn record_debug(&mut self, field: &Field, _value: &dyn std::fmt::Debug) {
        self.0.push(field.name().to_string());
    }
}

impl<S> Layer<S> for CaptureLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: Context<'_, S>,
    ) {
        let mut names = FieldNames(Vec::new());
        attrs.record(&mut names);
        self.out
            .lock()
            .unwrap()
            .spans
            .push((attrs.metadata().name().to_string(), names.0));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn request_span_carries_request_id() {
    let captured = Arc::new(Mutex::new(Captured::default()));
    let layer = CaptureLayer {
        out: captured.clone(),
    };
    let subscriber = Registry::default().with(layer);

    // Hold the default for this thread for the duration of the request.
    let _guard = tracing::subscriber::set_default(subscriber);

    let harness = support::TestHarness::new().await;
    let app = harness.router();
    let resp = app
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let spans = &captured.lock().unwrap().spans;
    let request_span = spans
        .iter()
        .find(|(name, _)| name == "request")
        .expect("a `request` span must be created per HTTP request");
    assert!(
        request_span.1.iter().any(|f| f == "request_id"),
        "request span must carry a request_id field, got {:?}",
        request_span.1
    );
}
