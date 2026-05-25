//! `POST /v1/sessions/:id/question/answer` — resolve a pending
//! `ask_user` question by routing the user's selected option indices
//! into the in-memory rendezvous registry.
//!
//! Single-use semantics: the registry removes the entry on first
//! resolve, so a replayed answer (e.g. retry after timeout) returns
//! 404 rather than re-firing the awaiting tool.
//!
//! Auth: the route requires an `AuthPrincipal` extension on the
//! request. Integrators (cloud plugin / reverse proxy middleware)
//! attach this marker via a tower layer; absence == 401. Core stays
//! auth-blind by construction — the marker is the *only* thing we
//! check, not its contents.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use openlet_core::runtime::question_registry::{QuestionId, ResolveError};
use openlet_protocol::QuestionAnswerDto;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

/// Marker the auth middleware inserts via `request.extensions_mut().insert`.
/// Cloud integrators may define their own concrete type and register a
/// tower layer that converts it to this marker; the route only cares
/// whether *some* principal is present, not what shape it has.
#[derive(Clone, Debug)]
pub struct AuthPrincipal;

#[utoipa::path(
    post,
    path = "/v1/sessions/{id}/question/answer",
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
    Path(_session_id): Path<Uuid>,
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
    state
        .questions
        .resolve(qid, body.selected)
        .map_err(map_resolve_err)?;
    Ok(StatusCode::OK)
}

fn map_resolve_err(e: ResolveError) -> AppError {
    match e {
        ResolveError::NotFound | ResolveError::ReceiverDropped => AppError::not_found(
            "question_not_found",
            "question not found or already resolved",
        ),
    }
}
