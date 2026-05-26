//! Rewrite a leading `@subagent_name objective…` text part into a
//! matching synthetic `subagent_task` tool call.
//!
//! Mid-prompt mentions and unknown slugs leave the parts untouched
//! (literal). Per F4.5, the parser is ASCII-only and rejects Unicode
//! confusables.
//!
//! Behaviour:
//!   - First text part is checked. If it parses as a valid mention
//!     against the live agent registry, an extra `Part::ToolCall` is
//!     appended carrying `subagent_task` with the resolved slug +
//!     objective.
//!   - The original text part is preserved so audit / SSE consumers
//!     still see what the user typed.
//!   - `background = false` (sync mode) — the user explicitly invoked
//!     the subagent and wants its result before the parent continues.

use openlet_core::types::part::Part;

use crate::app_state::AppState;

#[must_use]
pub(crate) fn rewrite_mention_into_subagent_task(parts: Vec<Part>, state: &AppState) -> Vec<Part> {
    let Some(first_text) = parts.iter().find_map(|p| match p {
        Part::Text { text, .. } => Some(text.clone()),
        _ => None,
    }) else {
        return parts;
    };
    let Some((slug, objective)) = openlet_core::runtime::subagent::parse_subagent_mention(
        &first_text,
        state.agent_registry.as_ref(),
    ) else {
        return parts;
    };
    let mut out = parts;
    let args = serde_json::json!({
        "subagent_type": slug.as_str(),
        "objective": objective,
        "background": false,
    });
    out.push(Part::ToolCall {
        id: openlet_core::types::part::PartId::new(),
        call_id: format!("mention-{}", uuid::Uuid::new_v4()),
        name: "subagent_task".to_string(),
        args,
    });
    out
}
