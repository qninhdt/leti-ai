//! Question DTOs — body shape for `POST /v1/session/:id/question/answer`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// `POST /v1/session/:id/question/answer` body. `question_id` matches
/// the UUIDv7 the `ask_user` tool minted when it suspended; `selected`
/// is the list of option indices the user picked (single-select carries
/// exactly one entry, multi-select may carry zero or more).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct QuestionAnswerDto {
    pub question_id: Uuid,
    #[serde(default)]
    pub selected: Vec<usize>,
}
