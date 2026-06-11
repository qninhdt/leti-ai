//! Live E2E harness — boots a fully-wired server on a real loopback TCP
//! port with the real `OpenAiCompatProvider`, then drives it over real
//! HTTP + SSE the way the TUI client does.
//!
//! This is the layer the existing `oneshot`/`StubProvider` integration
//! tests don't cover: a genuine runtime turn loop, streaming a real
//! provider's bytes through the processor into persisted parts and live
//! SSE frames.
//!
//! Two provider backends:
//! - `LiveServer::with_mock()` — points the provider at the in-process
//!   `MockOpenAiService` (deterministic, network-free, default CI path).
//! - `LiveServer::with_openrouter()` — points at real OpenRouter using
//!   `OPENROUTER_API_KEY`. Only constructed by `#[ignore]`d, env-gated
//!   tests so a keyless CI stays green.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openlet_adapters::bus::BroadcastBus;
use openlet_adapters::config_perm::ConfigPermissionMgr;
use openlet_adapters::localfs::{LocalFilesystem, LocalFsArtifactStore};
use openlet_adapters::localshell::{LocalShellExecutor, LocalShellToolExecutor};
use openlet_adapters::openrouter::OpenRouterProvider;
use openlet_adapters::sqlite::event_repo::SqliteEventRepo;
use openlet_adapters::sqlite::{SqliteMemoryStore, open_in_memory, open_pool, run_migrations};
use openlet_core::adapters::ModelProvider;
use openlet_core::config::{Config, LogFormat, PluginsConfig};
use openlet_core::runtime::{ConversationRuntime, RuntimeConfig};
use openlet_core::types::agent::{AgentId, AgentSpec};
use openlet_plugin_api::context::CoreApi;
use openlet_plugin_registry::{PluginRegistry, install_all};
use rust_decimal::Decimal;
use secrecy::SecretString;
use serde_json::Value;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// A running server bound to a real loopback port. Drop aborts the serve
/// task and releases the tempdir.
pub struct LiveServer {
    base_url: String,
    http: reqwest::Client,
    model: String,
    workspace_root: PathBuf,
    // The same store the running server uses. Exposed so the persistence
    // test can read persisted messages/parts back from the reopened on-disk
    // sqlite after a reboot (assistant text lives in the parts table, not in
    // the transient SSE delta stream, so there's no HTTP route to read it).
    memory: Arc<dyn openlet_core::adapters::MemoryStore>,
    serve_task: JoinHandle<()>,
    // `None` when the data dir is caller-owned (the persistence test passes
    // its own `TempDir` so the same dir survives a boot→drop→reboot cycle);
    // `Some` when boot created an ephemeral dir it should release on drop.
    _tempdir: Option<TempDir>,
}

impl Drop for LiveServer {
    fn drop(&mut self) {
        self.serve_task.abort();
    }
}

impl LiveServer {
    /// Boot against the in-process mock provider. `model` selects nothing
    /// upstream (the mock is body-driven) but is threaded through as the
    /// session's default model for realism. In-memory sqlite (fast default).
    pub async fn with_mock(base_url: &str) -> Self {
        Self::boot(
            base_url,
            Some(SecretString::from("test-key")),
            "mock/model-small",
            None,
            None,
        )
        .await
    }

    /// Boot against the mock provider with an **on-disk** sqlite at
    /// `<data_dir>/db.sqlite`, reusing the caller-supplied dir. The default
    /// `boot` uses `open_in_memory()` (verified faster for one-shot tests),
    /// so plain persistence across a process restart can only be exercised
    /// via this on-disk path. The caller owns `data_dir` (typically a
    /// `TempDir`) so it survives a boot→drop→reboot cycle on the same dir.
    pub async fn with_mock_on_disk(base_url: &str, data_dir: PathBuf) -> Self {
        Self::boot(
            base_url,
            Some(SecretString::from("test-key")),
            "mock/model-small",
            Some(data_dir),
            None,
        )
        .await
    }

    /// Boot against real OpenRouter. Reads `OPENROUTER_API_KEY` from env;
    /// callers must gate on its presence + `OPENLET_LIVE_E2E=1`.
    pub async fn with_openrouter() -> Self {
        Self::with_openrouter_inner(None).await
    }

    /// Boot against real OpenRouter with a deliberately small agent context
    /// window so a multi-turn conversation crosses the compaction threshold.
    /// Registers a `general` agent (the harness default slug) carrying the
    /// given `context_window`; without a registered agent, `loop_ctx.agent`
    /// is `None` and compaction never fires. Used by the compaction-continuity
    /// live test.
    pub async fn with_openrouter_small_window(context_window: u32) -> Self {
        Self::with_openrouter_inner(Some(context_window)).await
    }

    async fn with_openrouter_inner(compaction_window: Option<u32>) -> Self {
        let key = std::env::var("OPENROUTER_API_KEY")
            .expect("OPENROUTER_API_KEY required for live OpenRouter E2E");
        let model = std::env::var("OPENLET_LIVE_E2E_MODEL")
            .unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
        Self::boot(
            openlet_adapters::openrouter::DEFAULT_BASE_URL,
            Some(SecretString::from(key)),
            &model,
            None,
            compaction_window,
        )
        .await
    }

    /// `data_dir`: `Some` → on-disk sqlite reused across boots (persistence
    /// test); `None` → ephemeral tempdir + in-memory sqlite (fast default).
    async fn boot(
        base_url: &str,
        api_key: Option<SecretString>,
        model: &str,
        data_dir: Option<PathBuf>,
        compaction_window: Option<u32>,
    ) -> Self {
        let (data_root, owned_tempdir) = match data_dir {
            Some(d) => (d, None),
            None => {
                let td = tempfile::tempdir().expect("tempdir");
                (td.path().to_path_buf(), Some(td))
            }
        };
        let persistent = owned_tempdir.is_none();
        let workspace_root = data_root.join("ws");
        let artifact_root = data_root.join("artifacts");
        tokio::fs::create_dir_all(&workspace_root).await.unwrap();
        tokio::fs::create_dir_all(&artifact_root).await.unwrap();

        // On-disk pool when the caller owns the data dir (persistence test);
        // in-memory otherwise. open_in_memory() never persists across boots,
        // so the restart assertion REQUIRES the on-disk branch.
        let pool = if persistent {
            let db_path = data_root.join("db.sqlite");
            let p = open_pool(&db_path, 4).await.expect("open on-disk sqlite");
            run_migrations(&p).await.expect("migrations");
            p
        } else {
            open_in_memory().await.expect("sqlite")
        };
        let event_repo = SqliteEventRepo::new(pool.clone());
        let memory: Arc<dyn openlet_core::adapters::MemoryStore> =
            Arc::new(SqliteMemoryStore::new(pool.clone()));
        let events: Arc<dyn openlet_core::adapters::EventSink> =
            Arc::new(BroadcastBus::with_repo(event_repo));

        let provider: Arc<dyn ModelProvider> = Arc::new(OpenRouterProvider::new(
            base_url.to_string(),
            api_key,
            openlet_adapters::openrouter::OpenRouterConfig::default(),
        ));

        let config = Config {
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: data_root.clone(),
            openrouter_api_key: None,
            default_model: model.to_string(),
            permission_ruleset_path: None,
            log_format: LogFormat::Pretty,
            plugins: PluginsConfig::default(),
        };

        let runtime = Arc::new(ConversationRuntime::new(
            provider.clone(),
            memory.clone(),
            events.clone(),
            RuntimeConfig::new(model.to_string()),
        ));

        let shell_exec = Arc::new(LocalShellExecutor::new(workspace_root.clone()));
        let fs_adapter = Arc::new(LocalFilesystem::new(workspace_root.clone()));
        let shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor> = shell_exec.clone();

        let core_api: Arc<dyn CoreApi> = Arc::new(NoopCoreApi);
        let task_registry = Arc::new(openlet_core::runtime::subagent::TaskRegistry::new(32));
        let spawner: Arc<dyn openlet_core::tools::builtins::subagent_task::SubagentSpawner> =
            Arc::new(StubSubagentSpawner);
        let plugins = openlet_plugin_registry::all_plugins(
            shell.clone(),
            memory.clone(),
            task_registry.clone(),
            spawner,
        );
        let configs = HashMap::new();
        let installed = install_all(plugins, &configs, core_api)
            .await
            .expect("install plugins");

        let mut tool_builder = openlet_core::tools::ToolRegistry::builder();
        for tool in installed.tools {
            tool_builder = tool_builder.register_erased(tool);
        }
        let tool_registry = tool_builder.build();

        // Populate the plugin registry the same way the binary does so
        // `GET /v1/plugin*` serves the real registered set, not an empty one.
        let mut plugin_registry = PluginRegistry::new();
        for plugin in installed.plugins {
            plugin_registry.push(plugin);
        }

        let default_agent_id = AgentId::new();
        let agent_spec = AgentSpec::new(default_agent_id, workspace_root.clone(), "default");
        let mut agents: HashMap<AgentId, openlet_server::AgentResources> = HashMap::new();
        agents.insert(
            default_agent_id,
            openlet_server::AgentResources {
                spec: agent_spec,
                fs: fs_adapter.clone(),
                shell: shell.clone(),
            },
        );

        // Register a `general` agent (the harness default session slug) only
        // when a caller asks for a small compaction window. Without a
        // registered agent, `loop_ctx.agent` is `None` and compaction never
        // fires — so the compaction-continuity test opts in here.
        let mut agent_registry = openlet_core::agent::AgentRegistry::new();
        if let Some(window) = compaction_window {
            use openlet_core::agent::{AgentDefinition, AgentSlug, PromptSegments};
            let def = AgentDefinition {
                slug: AgentSlug::new("general").expect("slug"),
                title: "General".into(),
                description: String::new(),
                prompt_segments: Some(PromptSegments::default()),
                tool_allowlist: Vec::new(),
                model_id: model.to_string(),
                default_temperature: 0.0,
                context_window: window,
                compaction_threshold: 0.5,
                compaction_summary_cap_tokens: 500,
                hidden: false,
            };
            def.validate().expect("valid compaction agent");
            agent_registry.insert(def).expect("insert general agent");
        }

        let state = openlet_server::AppStateBuilder::new()
            .provider(provider)
            .memory(memory.clone())
            .artifacts(Arc::new(LocalFsArtifactStore::new(
                artifact_root,
                pool.clone(),
            )))
            .tools(Arc::new(LocalShellToolExecutor::new()))
            .tool_registry(tool_registry)
            .events(events)
            .permission(Arc::new(ConfigPermissionMgr::new()))
            .config(Arc::new(config))
            .plugin_registry(Arc::new(plugin_registry))
            .runtime(runtime)
            .agents(agents)
            .default_agent_id(default_agent_id)
            .agent_registry(Arc::new(agent_registry))
            .build()
            .expect("build app state");

        // Mirror the binary (`main.rs`): the question/answer route hard-requires
        // an `AuthPrincipal` extension. Without this layer a real model's
        // `ask_user` answer POST → 401 → the registry oneshot never resolves →
        // the runner hangs to its 300s timeout. Inject it like the binary does.
        let app = openlet_server::build_router(state)
            .layer(axum::Extension(openlet_server::AuthPrincipal::user("test")));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let base = format!("http://{addr}");

        let serve_task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");

        Self {
            base_url: base,
            http,
            model: model.to_string(),
            workspace_root,
            memory,
            serve_task,
            _tempdir: owned_tempdir,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Agent workspace root (`<data_dir>/ws`). The fs-write test asserts the
    /// scenario's file landed on real disk under here.
    pub fn workspace_root(&self) -> &std::path::Path {
        &self.workspace_root
    }

    /// The live server's memory store, backed by the same sqlite pool the
    /// HTTP API uses. The persistence test reads persisted messages/parts
    /// directly from here after a reboot on the same on-disk data dir —
    /// `part.delta` events are transient (verified `turn_stream.rs`), so the
    /// final assistant text lives only in the parts table, not the event log.
    pub fn memory(&self) -> &Arc<dyn openlet_core::adapters::MemoryStore> {
        &self.memory
    }

    /// `GET /v1/health` → status code.
    pub async fn health(&self) -> reqwest::StatusCode {
        self.http
            .get(format!("{}/v1/health", self.base_url))
            .send()
            .await
            .expect("health")
            .status()
    }

    /// `POST /v1/session` with an empty Cloud-shape body → session id.
    pub async fn create_session(&self) -> String {
        let body = serde_json::json!({
            "agent_id": null,
            "parent_session_id": null,
            "permission_mode": null,
            "extensions": null,
        });
        let resp = self
            .http
            .post(format!("{}/v1/session", self.base_url))
            .json(&body)
            .send()
            .await
            .expect("create session");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::CREATED,
            "session create"
        );
        let v: Value = resp.json().await.expect("session json");
        v["id"].as_str().expect("session id").to_string()
    }

    /// `POST /v1/session/:id/prompt_async` with one text part. Returns the
    /// 202-ack message id.
    pub async fn prompt(&self, session_id: &str, text: &str) -> reqwest::StatusCode {
        let body = serde_json::json!({
            "parts": [{
                "kind": "text",
                "id": uuid::Uuid::new_v4(),
                "text": text,
            }]
        });
        self.http
            .post(format!(
                "{}/v1/session/{session_id}/prompt_async",
                self.base_url
            ))
            .json(&body)
            .send()
            .await
            .expect("prompt_async")
            .status()
    }

    /// `POST /v1/session/:id/mode` — set the session permission mode
    /// (e.g. `"danger"` to auto-allow tool calls that would otherwise park
    /// on an Ask under the default `WorkspaceWrite`).
    pub async fn set_mode(&self, session_id: &str, mode: &str) -> reqwest::StatusCode {
        self.http
            .post(format!("{}/v1/session/{session_id}/mode", self.base_url))
            .json(&serde_json::json!({ "mode": mode }))
            .send()
            .await
            .expect("set_mode")
            .status()
    }

    /// `POST /v1/session/:id/abort`.
    pub async fn abort(&self, session_id: &str) -> reqwest::StatusCode {
        self.http
            .post(format!("{}/v1/session/{session_id}/abort", self.base_url))
            .send()
            .await
            .expect("abort")
            .status()
    }

    /// `GET {path}` → decoded JSON value (asserts 200).
    pub async fn get_json(&self, path: &str) -> Value {
        let resp = self
            .http
            .get(format!("{}{path}", self.base_url))
            .send()
            .await
            .expect("get");
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET {path}");
        resp.json().await.expect("json")
    }

    /// `GET {path}` → (status, decoded JSON body). Does not assert status,
    /// so callers can check both 2xx and error responses + their slug body.
    pub async fn get_with_status(&self, path: &str) -> (reqwest::StatusCode, Value) {
        let resp = self
            .http
            .get(format!("{}{path}", self.base_url))
            .send()
            .await
            .expect("get");
        let status = resp.status();
        let body = resp.json().await.unwrap_or(Value::Null);
        (status, body)
    }

    /// `GET /v1/models` → decoded JSON array.
    pub async fn models(&self) -> Vec<Value> {
        let resp = self
            .http
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await
            .expect("models");
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        resp.json().await.expect("models json")
    }

    /// Subscribe to the session SSE stream and collect frames until a
    /// terminal `session_status` (idle/cancelled/errored) is seen or the
    /// deadline elapses. Returns the parsed `data:` JSON of every frame.
    ///
    /// This mirrors what the TUI's `connectSse` does: open the channel,
    /// accumulate `message_created` / `part_created` / `part_delta` /
    /// `part_updated`, stop on terminal status.
    pub async fn collect_session_events(&self, session_id: &str, deadline: Duration) -> Vec<Value> {
        use futures::StreamExt as _;

        let url = format!("{}/v1/event?session={session_id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("accept", "text/event-stream")
            .send()
            .await
            .expect("sse connect");
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "sse status");

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut frames: Vec<Value> = Vec::new();

        let collect = async {
            while let Some(chunk) = stream.next().await {
                let Ok(bytes) = chunk else { break };
                buf.push_str(&String::from_utf8_lossy(&bytes));

                // SSE frames are separated by a blank line. Drain complete
                // frames out of the buffer as they arrive.
                while let Some(idx) = buf.find("\n\n") {
                    let raw = buf[..idx].to_string();
                    buf.drain(..idx + 2);
                    if let Some(data) = parse_sse_data(&raw) {
                        if let Ok(json) = serde_json::from_str::<Value>(&data) {
                            let terminal = is_terminal_status(&json);
                            frames.push(json);
                            if terminal {
                                return;
                            }
                        }
                    }
                }
            }
        };

        let _ = tokio::time::timeout(deadline, collect).await;
        frames
    }
}

/// Extract the joined `data:` payload from one raw SSE frame (handles
/// multi-line `data:` per the SSE spec; ignores `id:`/`event:` lines).
fn parse_sse_data(raw: &str) -> Option<String> {
    let mut data_lines: Vec<&str> = Vec::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

/// True when a frame is a `session_status` event in a terminal state.
fn is_terminal_status(json: &Value) -> bool {
    json.get("kind").and_then(Value::as_str) == Some("session_status")
        && matches!(
            json.get("status").and_then(Value::as_str),
            Some("idle" | "cancelled" | "errored")
        )
}

// --- Inline stubs (mirrors support.rs; kept local so this harness is
// self-contained and doesn't couple to the oneshot harness module). ---

struct NoopCoreApi;

#[async_trait]
impl CoreApi for NoopCoreApi {
    async fn current_session_meta(
        &self,
        _: openlet_core::types::session::SessionId,
    ) -> Option<openlet_core::types::session::SessionMeta> {
        None
    }
    fn session_cost(&self, _: openlet_core::types::session::SessionId) -> Decimal {
        Decimal::ZERO
    }
    fn record_cost(&self, _: openlet_core::types::session::SessionId, _: Decimal) {}
    async fn emit_event(
        &self,
        _: openlet_core::types::event::AgentEvent,
        _: openlet_core::adapters::event_sink::Persistence,
    ) {
    }
    fn read_config(&self, _: &str) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn cancel_session(&self, _: openlet_core::types::session::SessionId, _: String) {}
    async fn emit_notification(
        &self,
        _: Option<openlet_core::types::session::SessionId>,
        _: openlet_core::hooks::io::NotificationLevel,
        _: String,
        _: String,
        _: String,
    ) {
    }
}

struct StubSubagentSpawner;

#[async_trait]
impl openlet_core::tools::builtins::subagent_task::SubagentSpawner for StubSubagentSpawner {
    async fn spawn(
        &self,
        _ctx: &openlet_core::adapters::tool_executor::ToolCtx,
        _subagent_type: &str,
        _objective: &str,
    ) -> Result<openlet_core::runtime::subagent::TaskId, openlet_core::runtime::subagent::SpawnError>
    {
        Err(openlet_core::runtime::subagent::SpawnError::Internal(
            "stub spawner".into(),
        ))
    }
    async fn await_completion(
        &self,
        _task_id: openlet_core::runtime::subagent::TaskId,
    ) -> Result<
        (
            String,
            Option<String>,
            openlet_core::runtime::subagent::TaskStatus,
        ),
        openlet_core::runtime::subagent::SpawnError,
    > {
        Err(openlet_core::runtime::subagent::SpawnError::Internal(
            "stub spawner".into(),
        ))
    }
}
