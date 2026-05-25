//! Model-reachability probe: tries `GET <base>/models` first (free for
//! OpenAI/Anthropic/Gemini), falls back to anonymous `HEAD <base>` so
//! 401 still indicates the host is reachable. NEVER calls chat
//! completion — burning user quota on a health check is a bug.

use std::time::{Duration, Instant};

use reqwest::Client;
use secrecy::ExposeSecret;
use tokio::time::timeout;

use super::{CheckResult, PER_CHECK_BUDGET, Status, finish};
use crate::app_state::AppState;

const BASE_URL_ENV: &str = "OPENLET_MODEL_BASE_URL";

pub(super) async fn check_model_reachable(state: &AppState) -> CheckResult {
    let start = Instant::now();
    let Ok(base) = std::env::var(BASE_URL_ENV) else {
        return finish(
            "model_reachable",
            start,
            Status::Degraded,
            Some("no base URL configured".into()),
        );
    };
    let trimmed = base.trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return finish(
            "model_reachable",
            start,
            Status::Degraded,
            Some("no base URL configured".into()),
        );
    }

    let api_key = state
        .config
        .openrouter_api_key
        .as_ref()
        .map(|k| k.expose_secret().to_string());

    let client = match Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(PER_CHECK_BUDGET)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return finish(
                "model_reachable",
                start,
                Status::Failed,
                Some(format!("http client build: {e}")),
            );
        }
    };

    // Step 1: GET /models with API key. Most providers expose this
    // free of charge; a 200 is a strong signal.
    let url = format!("{trimmed}/models");
    let mut req = client.get(&url);
    if let Some(ref key) = api_key {
        req = req.bearer_auth(key);
    }
    if let Ok(Ok(resp)) = timeout(PER_CHECK_BUDGET, req.send()).await {
        if resp.status().is_success() {
            return finish(
                "model_reachable",
                start,
                Status::Healthy,
                Some(format!("GET /models -> {}", resp.status().as_u16())),
            );
        }
        // 401 here means the host is reachable but the key is bad;
        // fall through to the anonymous HEAD step which treats 401 as
        // a positive reachability signal.
    }

    // Step 2: anonymous HEAD on the base URL. 401 / 403 / 404 still
    // mean a server is on the other end of the wire — that's the only
    // signal /doctor actually needs.
    let head_req = client.head(&trimmed);
    match timeout(PER_CHECK_BUDGET, head_req.send()).await {
        Ok(Ok(resp)) => {
            let code = resp.status().as_u16();
            // Any HTTP status (incl. 4xx) means we got a response from
            // the upstream — network + DNS + TLS work. 5xx is treated
            // as Degraded so on-call sees it without flipping overall
            // to Failed (transient upstream).
            if (200..500).contains(&code) {
                finish(
                    "model_reachable",
                    start,
                    Status::Healthy,
                    Some(format!("HEAD {trimmed} -> {code}")),
                )
            } else {
                finish(
                    "model_reachable",
                    start,
                    Status::Degraded,
                    Some(format!("HEAD {trimmed} -> {code}")),
                )
            }
        }
        Ok(Err(e)) => finish(
            "model_reachable",
            start,
            Status::Failed,
            Some(format!("HEAD {trimmed} failed: {e}")),
        ),
        Err(_) => finish(
            "model_reachable",
            start,
            Status::Failed,
            Some("timed out".into()),
        ),
    }
}
