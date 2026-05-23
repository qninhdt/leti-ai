//! JSONL session log tests — redaction + rotation.

use chrono::Utc;
use openlet_adapters::localfs::SessionLogger;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::{SessionId, SessionStatus};

#[tokio::test]
async fn appends_one_line_per_event() {
    let dir = tempfile::tempdir().unwrap();
    let logger = SessionLogger::new(dir.path().to_path_buf());
    let session = SessionId::new();

    for _ in 0..3 {
        let ev = AgentEvent::SessionStatus {
            session_id: session,
            status: SessionStatus::Idle,
            at: Utc::now(),
        };
        logger.append(session, &ev).await.unwrap();
    }

    let path = dir.path().join(format!("{session}.jsonl"));
    let body = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(body.lines().count(), 3);
}

#[tokio::test]
async fn redacts_sensitive_keys_and_bearer_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let logger = SessionLogger::new(dir.path().to_path_buf());
    let session = SessionId::new();

    let ev = AgentEvent::Error {
        session_id: Some(session),
        code: "auth".into(),
        message: "Authorization: Bearer sk-abc1234567890ABCDEFG failed".into(),
    };
    logger.append(session, &ev).await.unwrap();

    let body = tokio::fs::read_to_string(dir.path().join(format!("{session}.jsonl")))
        .await
        .unwrap();
    assert!(body.contains("<redacted>"), "expected redaction marker: {body}");
    assert!(!body.contains("sk-abc1234567890ABCDEFG"));
}
