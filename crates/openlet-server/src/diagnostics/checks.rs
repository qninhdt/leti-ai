//! Per-check implementations split from `diagnostics.rs` so they stay
//! testable in isolation and the parent module stays under 200 lines.

use std::path::Path;
use std::time::Instant;

use openlet_core::types::session::SessionFilter;
use secrecy::{ExposeSecret, SecretString};
use tokio::net::TcpListener;
use tokio::time::timeout;

use super::{CheckResult, PER_CHECK_BUDGET, Status, finish};
use crate::app_state::AppState;

/// Pure inspection of the api-key value. Split out so unit tests can
/// exercise both branches without spinning up an `AppState`.
pub(super) fn api_key_status(key: Option<&SecretString>) -> (Status, Option<String>) {
    match key {
        Some(k) if !k.expose_secret().trim().is_empty() => (Status::Healthy, None),
        Some(_) => (
            Status::Degraded,
            Some("OPENROUTER_API_KEY is set but empty".into()),
        ),
        None => (Status::Degraded, Some("OPENROUTER_API_KEY not set".into())),
    }
}

pub(super) fn check_api_key_set(state: &AppState) -> CheckResult {
    let start = Instant::now();
    let (status, detail) = api_key_status(state.config.openrouter_api_key.as_ref());
    finish("api_key_set", start, status, detail)
}

/// Try to create + delete a probe file under `dir`. Pure I/O, broken out
/// so tests can target it directly with `tempfile`.
pub(super) async fn run_data_dir_probe(dir: &Path) -> std::io::Result<()> {
    let probe = dir.join(format!(".doctor-probe-{}", uuid::Uuid::new_v4()));
    tokio::fs::write(&probe, b"probe").await?;
    tokio::fs::remove_file(&probe).await?;
    Ok(())
}

pub(super) fn data_dir_status(result: std::io::Result<()>) -> (Status, Option<String>) {
    match result {
        Ok(()) => (Status::Healthy, None),
        Err(e) => (Status::Failed, Some(e.to_string())),
    }
}

pub(super) async fn check_data_dir_writable(state: &AppState) -> CheckResult {
    let start = Instant::now();
    let dir = state.config.data_dir.clone();
    let probe = timeout(
        PER_CHECK_BUDGET,
        async move { run_data_dir_probe(&dir).await },
    )
    .await;
    let (status, detail) = match probe {
        Ok(inner) => data_dir_status(inner),
        Err(_) => (Status::Failed, Some("timed out".into())),
    };
    finish("data_dir_writable", start, status, detail)
}

pub(super) async fn check_sqlite_health(state: &AppState) -> CheckResult {
    let start = Instant::now();
    // Proxy for `SELECT 1`: list_sessions exercises the same connection
    // pool + query path without requiring a new trait method.
    let result = timeout(
        PER_CHECK_BUDGET,
        state.memory.list_sessions(SessionFilter::default()),
    )
    .await;
    let (status, detail) = match result {
        Ok(Ok(_)) => (Status::Healthy, None),
        Ok(Err(e)) => (Status::Failed, Some(e.to_string())),
        Err(_) => (Status::Failed, Some("timed out".into())),
    };
    finish("sqlite_health", start, status, detail)
}

pub(super) fn plugin_count_status(count: usize) -> (Status, Option<String>) {
    if count == 0 {
        (
            Status::Degraded,
            Some("no plugins exposed via state.plugin_registry".into()),
        )
    } else {
        (
            Status::Healthy,
            Some(format!("{count} plugin(s) registered")),
        )
    }
}

pub(super) fn check_plugin_lifecycle(state: &AppState) -> CheckResult {
    let start = Instant::now();
    let count = state.plugin_registry.iter().count();
    let (status, detail) = plugin_count_status(count);
    finish("plugin_lifecycle", start, status, detail)
}

pub(super) async fn check_port_free(state: &AppState) -> CheckResult {
    let start = Instant::now();
    let addr = state.config.bind_addr.clone();
    let result = timeout(PER_CHECK_BUDGET, TcpListener::bind(&addr)).await;
    let (status, detail) = match result {
        Ok(Ok(listener)) => {
            // Drop immediately — the bind probe is the whole check.
            drop(listener);
            (Status::Healthy, None)
        }
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::AddrInUse => (
            Status::Degraded,
            Some(format!("{addr} already in use (likely an existing server)")),
        ),
        Ok(Err(e)) => (Status::Failed, Some(e.to_string())),
        Err(_) => (Status::Failed, Some("timed out".into())),
    };
    finish("port_free", start, status, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_healthy_when_present_and_non_empty() {
        let key = SecretString::from("sk-secret-value-12345".to_string());
        let (status, detail) = api_key_status(Some(&key));
        assert_eq!(status, Status::Healthy);
        assert!(detail.is_none());

        // Critical: the secret value MUST NOT appear in the result. The
        // CheckResult derives Debug; render it and grep for the value.
        let result = CheckResult {
            name: "api_key_set",
            status,
            detail,
            elapsed_ms: 0,
        };
        let rendered = format!("{result:?}");
        assert!(
            !rendered.contains("sk-secret-value-12345"),
            "secret leaked into Debug output: {rendered}"
        );
    }

    #[test]
    fn api_key_degraded_when_absent() {
        let (status, detail) = api_key_status(None);
        assert_eq!(status, Status::Degraded);
        assert!(detail.is_some_and(|d| d.contains("OPENROUTER_API_KEY")));
    }

    #[test]
    fn api_key_degraded_when_blank_only() {
        let blank = SecretString::from("   ".to_string());
        let (status, _) = api_key_status(Some(&blank));
        assert_eq!(status, Status::Degraded);
    }

    #[tokio::test]
    async fn data_dir_writable_on_tempdir() {
        let tmp = tempfile::tempdir().unwrap();
        let result = run_data_dir_probe(tmp.path()).await;
        let (status, _) = data_dir_status(result);
        assert_eq!(status, Status::Healthy);
    }

    #[tokio::test]
    async fn data_dir_failed_on_nonexistent_path() {
        // No tempfile, no chroot — just point at a path the OS can't
        // create files in. A nested path under a regular file is the
        // most portable failure mode (Linux + macOS both error
        // immediately on parent-not-a-dir).
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let bogus = tmp.path().join("nested/cannot/exist");
        let result = run_data_dir_probe(&bogus).await;
        let (status, detail) = data_dir_status(result);
        assert_eq!(status, Status::Failed);
        assert!(detail.is_some());
    }

    #[test]
    fn plugin_count_zero_is_degraded() {
        let (status, _) = plugin_count_status(0);
        assert_eq!(status, Status::Degraded);
    }

    #[test]
    fn plugin_count_nonzero_is_healthy() {
        let (status, detail) = plugin_count_status(3);
        assert_eq!(status, Status::Healthy);
        assert!(detail.unwrap().contains("3 plugin"));
    }
}
