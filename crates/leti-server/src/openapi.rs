//! utoipa OpenAPI aggregator.
//!
//! Each route module exposes `#[utoipa::path]`-annotated handlers; the
//! `OpenApi` derive below collects them into the `/doc/openapi.json` doc.

use leti_protocol::{
    AbortAckDto, AgentDto, AskOptionDto, AttachmentKindDto, BackgroundTaskAckDto, CreateMessageDto,
    CreateSessionDto, DeltaKindDto, ErrorDto, EventDto, HealthDto, MessageDto, ModelDto,
    NotificationLevelDto, PartDto, PermissionDecisionDto, PermissionReplyDto, PermissionReplyKind,
    PermissionRequestDto, PromptAckDto, SessionDto, SetModeDto, UsageDto, dto::QuestionAnswerDto,
};
use utoipa::OpenApi;

use crate::diagnostics::{CheckResult, DoctorReport, Status};
use crate::routes::attachments::AttachmentAck;
use crate::routes::files::{FileContentDto, FileEntryDto, FileKindDto, FileListDto};
use crate::routes::plugin::{PluginHealthDto, PluginInfoDto};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Leti Agent Core API",
        version = "0.1.0",
        description = "REST + SSE surface for the Leti agent runtime.",
        license(name = "Apache-2.0")
    ),
    components(schemas(
        AbortAckDto,
        BackgroundTaskAckDto,
        AgentDto,
        AskOptionDto,
        AttachmentAck,
        AttachmentKindDto,
        CheckResult,
        CreateMessageDto,
        CreateSessionDto,
        DeltaKindDto,
        DoctorReport,
        ErrorDto,
        EventDto,
        FileContentDto,
        FileEntryDto,
        FileKindDto,
        FileListDto,
        HealthDto,
        MessageDto,
        ModelDto,
        NotificationLevelDto,
        PartDto,
        PermissionDecisionDto,
        PermissionReplyDto,
        PermissionReplyKind,
        PermissionRequestDto,
        PluginHealthDto,
        PluginInfoDto,
        PromptAckDto,
        QuestionAnswerDto,
        SessionDto,
        SetModeDto,
        Status,
        UsageDto,
    )),
    tags(
        (name = "global",      description = "Server-wide endpoints (health, version)"),
        (name = "session",     description = "Session lifecycle + prompt dispatch"),
        (name = "agent",       description = "Registered agent inventory"),
        (name = "permission",  description = "Permission ask/reply flow"),
        (name = "question",    description = "Interactive ask_user prompt replies"),
        (name = "event",       description = "SSE event channel"),
        (name = "plugin",      description = "Plugin discovery + health"),
        (name = "files",       description = "Workspace file listing + content (@-mentions)"),
        (name = "diagnostics", description = "Preflight diagnostics"),
    )
)]
pub struct ApiDoc;
