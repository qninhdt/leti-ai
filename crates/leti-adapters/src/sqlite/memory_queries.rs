//! Shared SQL filter-clause builders for `SqliteMemoryStore`.
//!
//! Both paginated and non-paginated session/message queries delegate
//! filter construction here to avoid duplicating WHERE-clause logic.

use leti_core::types::session::SessionFilter;

use super::codec::status_str;

/// Builds the WHERE clause fragments for session listing and binds the
/// filter values. Returns the SQL suffix (starting from " AND ...") and
/// a list of bind values in order.
pub(crate) fn session_filter_clause(filter: &SessionFilter) -> (String, Vec<String>) {
    let mut clauses = String::new();
    let mut binds: Vec<String> = Vec::new();

    if !filter.include_deleted {
        clauses.push_str(" AND deleted_at IS NULL");
    }
    if let Some(s) = filter.status {
        clauses.push_str(" AND status = ?");
        binds.push(status_str(s).to_string());
    }
    if let Some(ref a) = filter.agent_id {
        clauses.push_str(" AND agent_id = ?");
        binds.push(a.to_string());
    }

    (clauses, binds)
}
