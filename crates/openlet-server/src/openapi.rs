//! utoipa OpenAPI aggregator.
//!
//! Each route module exposes `#[utoipa::path]`-annotated handlers; the
//! `OpenApi` derive below collects them into the `/doc/openapi.json` doc.

use openlet_protocol::{
    AbortAckDto, AgentDto, CreateMessageDto, CreateSessionDto, DeltaKindDto, ErrorDto, EventDto,
    HealthDto, MessageDto, PartDto, PermissionReplyDto, PermissionReplyKind, PermissionRequestDto,
    PromptAckDto, SessionDto, SetModeDto, UsageDto,
};
use utoipa::OpenApi;

use crate::routes::plugin::{PluginHealthDto, PluginInfoDto};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Openlet Agent Core API",
        version = "0.1.0",
        description = "REST + SSE surface for the Openlet agent runtime.",
        license(name = "Apache-2.0")
    ),
    components(schemas(
        AbortAckDto,
        AgentDto,
        CreateMessageDto,
        CreateSessionDto,
        DeltaKindDto,
        ErrorDto,
        EventDto,
        HealthDto,
        MessageDto,
        PartDto,
        PermissionReplyDto,
        PermissionReplyKind,
        PermissionRequestDto,
        PluginHealthDto,
        PluginInfoDto,
        PromptAckDto,
        SessionDto,
        SetModeDto,
        UsageDto,
    )),
    tags(
        (name = "global",     description = "Server-wide endpoints (health, version)"),
        (name = "session",    description = "Session lifecycle + prompt dispatch"),
        (name = "agent",      description = "Registered agent inventory"),
        (name = "permission", description = "Permission ask/reply flow"),
        (name = "event",      description = "SSE event channel"),
        (name = "plugin",     description = "Plugin discovery + health"),
    )
)]
pub struct ApiDoc;
