//! Row → domain decoders for the sessions and messages tables.

use sqlx::Row;

use openlet_core::error::MemoryError;
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::{Message, MessageId};
use openlet_core::types::session::{SessionCapabilities, SessionId, SessionMeta};

use super::super::codec::{
    decode_json, from_ms, map_io, parse_mode, parse_role, parse_status, parse_uuid,
};

pub(super) fn row_to_session(row: sqlx::sqlite::SqliteRow) -> Result<SessionMeta, MemoryError> {
    let id_str: String = row.try_get("id").map_err(map_io)?;
    let agent_id_str: String = row.try_get("agent_id").map_err(map_io)?;
    let parent: Option<String> = row.try_get("parent_session_id").map_err(map_io)?;
    let status: String = row.try_get("status").map_err(map_io)?;
    let mode: String = row.try_get("permission_mode").map_err(map_io)?;
    let version: String = row.try_get("version").map_err(map_io)?;
    let created_at: i64 = row.try_get("created_at").map_err(map_io)?;
    let updated_at: i64 = row.try_get("updated_at").map_err(map_io)?;
    let deleted_at: Option<i64> = row.try_get("deleted_at").map_err(map_io)?;
    let extensions: String = row.try_get("extensions").map_err(map_io)?;
    let extensions = decode_json(&extensions, "extensions json")?;
    let capabilities: String = row.try_get("capabilities").map_err(map_io)?;
    let capabilities: SessionCapabilities = decode_json(&capabilities, "capabilities json")?;
    let current_agent_slug: Option<String> = row.try_get("current_agent_slug").map_err(map_io)?;
    let previous_agent_slug: Option<String> = row.try_get("previous_agent_slug").map_err(map_io)?;
    let depth: i64 = row.try_get("depth").map_err(map_io)?;
    let depth = u8::try_from(depth.max(0)).unwrap_or(u8::MAX);

    Ok(SessionMeta {
        id: SessionId(parse_uuid(&id_str)?),
        agent_id: AgentId(parse_uuid(&agent_id_str)?),
        status: parse_status(&status)?,
        permission_mode: parse_mode(&mode)?,
        parent_session_id: parent.map(|p| parse_uuid(&p).map(SessionId)).transpose()?,
        created_at: from_ms(created_at),
        updated_at: from_ms(updated_at),
        deleted_at: deleted_at.map(from_ms),
        version,
        extensions,
        capabilities,
        current_agent_slug,
        previous_agent_slug,
        depth,
    })
}

pub(super) fn row_to_message(row: sqlx::sqlite::SqliteRow) -> Result<Message, MemoryError> {
    let id_str: String = row.try_get("id").map_err(map_io)?;
    let session_str: String = row.try_get("session_id").map_err(map_io)?;
    let role: String = row.try_get("role").map_err(map_io)?;
    let created_at: i64 = row.try_get("created_at").map_err(map_io)?;

    Ok(Message {
        id: MessageId(parse_uuid(&id_str)?),
        session_id: SessionId(parse_uuid(&session_str)?),
        role: parse_role(&role)?,
        created_at: from_ms(created_at),
    })
}
