//! Metrics recorder + scrape endpoint.
//!
//! leti-ai is *software*, not an infra bundle: metric emission via the
//! `metrics` facade (`counter!`/`histogram!` at call sites) is a **no-op
//! until a recorder is installed**, and the `/metrics` endpoint only binds
//! when `LETI_METRICS_BIND` is set. Running `./leti-ai` locally needs
//! NO Prometheus, NO docker-compose — an operator who wants scraping points
//! their own Prometheus at the bind address.
//!
//! Security (M16): the open scrape exposes AGGREGATE metrics only — no
//! per-`workspace` label, because that label set enumerates every tenant
//! (cross-tenant spend leak + Prometheus cardinality DoS). Per-workspace
//! breakdown is deferred behind an authenticated admin scrape, not shipped
//! here. The endpoint also lives on a SEPARATE bind, never on the public
//! app router.

use std::net::SocketAddr;

use anyhow::Context;
use axum::Router;
use axum::routing::get;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tokio::net::TcpListener;
use tracing::info;

/// Env var selecting the metrics scrape bind address. Unset → metrics
/// fully dormant (no recorder, no endpoint).
pub const METRICS_BIND_ENV: &str = "LETI_METRICS_BIND";

/// Resolve the metrics bind address from the environment. `None` means
/// "off" — the default for a plain local run.
#[must_use]
pub fn metrics_bind_from_env() -> Option<String> {
    parse_metrics_bind(std::env::var(METRICS_BIND_ENV).ok())
}

/// Pure bind resolution: an unset or empty value is "off". Split out so
/// it's testable without mutating process env (the workspace forbids
/// `unsafe`, and `std::env::set_var` is unsafe in edition 2024).
#[must_use]
fn parse_metrics_bind(raw: Option<String>) -> Option<String> {
    raw.filter(|s| !s.is_empty())
}

/// Install the global Prometheus recorder and return its render handle.
/// Call this ONCE at boot, and ONLY when a metrics bind is configured —
/// without it, `metrics` macros compile to no-ops and cost nothing.
///
/// Returns an error if a recorder was already installed (double-install
/// is a boot bug, not a runtime condition).
pub fn install_recorder() -> anyhow::Result<PrometheusHandle> {
    PrometheusBuilder::new()
        .install_recorder()
        .context("installing Prometheus metrics recorder")
}

/// Serve `GET /metrics` on `bind` until the process exits. Spawned as a
/// detached task by the caller. The body is the recorder's text render
/// (Prometheus exposition format). Kept on its own listener so it is
/// never exposed through the authenticated app router or its body
/// limits/CORS.
pub async fn serve_metrics(bind: String, handle: PrometheusHandle) -> anyhow::Result<()> {
    let addr: SocketAddr = bind
        .parse()
        .with_context(|| format!("parsing {METRICS_BIND_ENV}={bind}"))?;

    let app = Router::new().route(
        "/metrics",
        get(move || {
            let handle = handle.clone();
            async move { handle.render() }
        }),
    );

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding metrics endpoint {addr}"))?;
    info!(bind = %addr, "metrics endpoint listening at http://{addr}/metrics");
    axum::serve(listener, app)
        .await
        .context("serving metrics endpoint")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_is_dormant() {
        // No bind → metrics fully off, so the local binary needs no infra.
        assert!(parse_metrics_bind(None).is_none());
    }

    #[test]
    fn empty_is_treated_as_off() {
        assert!(parse_metrics_bind(Some(String::new())).is_none());
    }

    #[test]
    fn set_bind_is_used() {
        assert_eq!(
            parse_metrics_bind(Some("127.0.0.1:9464".into())).as_deref(),
            Some("127.0.0.1:9464")
        );
    }
}
