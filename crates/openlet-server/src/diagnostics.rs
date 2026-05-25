//! `/doctor` preflight diagnostics — read-only health snapshot of every
//! adapter in [`AppState`]. Each check has a 2 s individual timeout; the
//! full report bounds at ~12 s in the worst case (6 checks × 2 s).
//!
//! Output is always run through [`SecretRedactor`] before serialization,
//! so a check `detail` accidentally echoing a config value cannot leak
//! token-shaped strings (`sk-…`, `Bearer …`, JWTs, etc.).

use std::time::{Duration, Instant};

use openlet_adapters::localfs::SecretRedactor;
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::app_state::AppState;

mod checks;
mod model_probe;

/// Per-check timeout. Sum of 6 checks ≤ 12 s — fits the 10 s "healthy
/// install" target with ~2 s of slack for the model probe (which has
/// its own internal fallback path that may double-spend the budget).
pub(crate) const PER_CHECK_BUDGET: Duration = Duration::from_secs(2);

/// Severity ladder: a single Failed flips overall to Failed; a single
/// Degraded (with no Failed) flips overall to Degraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Healthy,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CheckResult {
    pub name: &'static str,
    pub status: Status,
    pub detail: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DoctorReport {
    pub checks: Vec<CheckResult>,
    pub overall: Status,
}

impl DoctorReport {
    /// Serialize through the audit redactor. Callers MUST use this rather
    /// than `serde_json::to_value` directly so token-shaped strings and
    /// sensitive keys are scrubbed before they leave the process.
    #[must_use]
    pub fn to_redacted_json(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        SecretRedactor::default().redact_in_place(&mut value);
        value
    }

    /// Process exit code: 0 = Healthy, 1 = Degraded, 2 = Failed.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self.overall {
            Status::Healthy => 0,
            Status::Degraded => 1,
            Status::Failed => 2,
        }
    }
}

/// Run all preflight checks and roll the worst-case status to `overall`.
pub async fn run_diagnostics(state: &AppState) -> DoctorReport {
    let checks = vec![
        checks::check_api_key_set(state),
        checks::check_data_dir_writable(state).await,
        checks::check_sqlite_health(state).await,
        checks::check_plugin_lifecycle(state),
        model_probe::check_model_reachable(state).await,
        checks::check_port_free(state).await,
    ];
    let overall = checks
        .iter()
        .fold(Status::Healthy, |acc, c| worst(acc, c.status));
    DoctorReport { checks, overall }
}

pub(crate) fn worst(a: Status, b: Status) -> Status {
    match (a, b) {
        (Status::Failed, _) | (_, Status::Failed) => Status::Failed,
        (Status::Degraded, _) | (_, Status::Degraded) => Status::Degraded,
        _ => Status::Healthy,
    }
}

pub(crate) fn finish(
    name: &'static str,
    start: Instant,
    status: Status,
    detail: Option<String>,
) -> CheckResult {
    CheckResult {
        name,
        status,
        detail,
        elapsed_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redactor_strips_api_key_from_final_json() {
        // Build a report whose `detail` smuggles a token-shaped value
        // and a sensitive-key object — both must be redacted on output.
        let report = DoctorReport {
            checks: vec![CheckResult {
                name: "api_key_set",
                status: Status::Healthy,
                detail: Some("loaded sk-1234567890abcdefghij from env".into()),
                elapsed_ms: 1,
            }],
            overall: Status::Healthy,
        };
        let dumped = report.to_redacted_json().to_string();
        assert!(
            !dumped.contains("sk-1234567890abcdefghij"),
            "token-shaped string must be redacted: {dumped}"
        );
    }

    #[test]
    fn redactor_strips_sensitive_key_from_nested_value() {
        // Redactor walks objects too — if a future check accidentally
        // serializes the raw config blob, the `api_key` field still gets
        // scrubbed before output. Mirrors the audit redactor contract.
        let mut value = serde_json::json!({
            "checks": [{
                "name": "x",
                "status": "healthy",
                "detail": null,
                "elapsed_ms": 0,
                "api_key": "sk-this-is-a-secret-1234567890"
            }],
            "overall": "healthy"
        });
        SecretRedactor::default().redact_in_place(&mut value);
        let dumped = value.to_string();
        assert!(!dumped.contains("sk-this-is-a-secret"), "{dumped}");
    }

    #[test]
    fn worst_picks_failed_over_degraded_over_healthy() {
        assert_eq!(worst(Status::Healthy, Status::Healthy), Status::Healthy);
        assert_eq!(worst(Status::Healthy, Status::Degraded), Status::Degraded);
        assert_eq!(worst(Status::Degraded, Status::Healthy), Status::Degraded);
        assert_eq!(worst(Status::Degraded, Status::Failed), Status::Failed);
        assert_eq!(worst(Status::Failed, Status::Healthy), Status::Failed);
    }

    #[test]
    fn exit_code_matches_overall_status() {
        let mk = |s| DoctorReport {
            checks: vec![],
            overall: s,
        };
        assert_eq!(mk(Status::Healthy).exit_code(), 0);
        assert_eq!(mk(Status::Degraded).exit_code(), 1);
        assert_eq!(mk(Status::Failed).exit_code(), 2);
    }
}
