//! `POST /v1/session/:id/question/answer` — resolve a pending
//! `ask_user` question by routing the user's selected option indices
//! into the in-memory rendezvous registry.
//!
//! Single-use semantics: the registry removes the entry on first
//! resolve, so a replayed answer (e.g. retry after timeout) returns
//! 404 rather than re-firing the awaiting tool.
//!
//! Auth: the route requires the canonical [`AuthPrincipal`] extension on
//! the request — the same type the mounted `AuthLayer` injects and the
//! workspace gate checks. This route only asserts a principal is present
//! (presence == authenticated); it does not inspect its contents.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use openlet_core::runtime::question_registry::{QuestionId, ResolveError};
use openlet_protocol::QuestionAnswerDto;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::auth::AuthPrincipal;
use crate::error::AppError;

#[utoipa::path(
    post,
    path = "/v1/session/{id}/question/answer",
    tag = "question",
    params(("id" = Uuid, Path, description = "Session id")),
    request_body = QuestionAnswerDto,
    responses(
        (status = 200, description = "Answer accepted"),
        (status = 401, description = "Missing auth principal"),
        (status = 404, description = "Question not found or already resolved"),
    )
)]
pub async fn answer(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    principal: Option<Extension<AuthPrincipal>>,
    Json(body): Json<QuestionAnswerDto>,
) -> Result<StatusCode, AppError> {
    if principal.is_none() {
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "missing_auth_principal",
            "no auth principal attached to request",
        ));
    }

    let qid = QuestionId::from(body.question_id);
    let session = openlet_core::types::session::SessionId::from(session_id);
    state
        .questions
        .resolve(qid, session, body.selected)
        .map_err(map_resolve_err)?;
    Ok(StatusCode::OK)
}

fn map_resolve_err(e: ResolveError) -> AppError {
    match e {
        ResolveError::NotFound | ResolveError::ReceiverDropped => AppError::not_found(
            "question_not_found",
            "question not found or already resolved",
        ),
        ResolveError::SessionMismatch => AppError::not_found(
            "question_not_found",
            "question not found or already resolved",
        ),
    }
}
