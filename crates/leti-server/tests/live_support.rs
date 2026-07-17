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
//!   `OPENAI_API_KEY`. Reached only when the runtime env gate
//!   (`LETI_LIVE_E2E=1` + key present) is satisfied; otherwise scenario
//!   boots transparently fall back to the scripted mock so a keyless CI
//!   stays green. No `#[ignore]` — the env gate is the single source of
//!   truth.

#![allow(dead_code)]

use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use leti_adapters::bus::BroadcastBus;
use leti_adapters::config_perm::ConfigPermissionMgr;
use leti_adapters::localfs::{LocalFilesystem, LocalFsArtifactStore};
use leti_adapters::localshell::LocalShellExecutor;
use leti_adapters::openrouter::OpenRouterProvider;
use leti_adapters::sqlite::event_repo::SqliteEventRepo;
use leti_adapters::sqlite::{SqliteMemoryStore, open_in_memory, open_pool, run_migrations};
use leti_core::adapters::ModelProvider;
use leti_core::config::{Config, LogFormat, PluginsConfig};
use leti_core::runtime::{ConversationRuntime, RuntimeConfig};
use leti_core::types::agent::{AgentId, AgentSpec};
use leti_plugin_api::context::CoreApi;
use leti_plugin_registry::{PluginHandles, install_all};
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
    memory: Arc<dyn leti_core::adapters::MemoryStore>,
    // The default agent id the server registered. Exposed so the ask_user
    // test can persist a question-capable session directly via the memory
    // store (bypassing the HTTP route), keeping the test hermetic regardless
    // of the route's default capabilities.
    default_agent_id: leti_core::types::agent::AgentId,
    // The artifact store the server uses. Exposed so the todo test can read
    // back the persisted `todos.json` the `todo` tool writes.
    artifacts: Arc<dyn leti_core::adapters::ArtifactStore>,
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
            false,
            Vec::new(),
            false,
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
            false,
            Vec::new(),
            false,
            None,
        )
        .await
    }

    /// Boot against real OpenRouter. Reads `OPENAI_API_KEY` from env;
    /// callers must gate on its presence + `LETI_LIVE_E2E=1`.
    pub async fn with_openrouter() -> Self {
        Self::with_openrouter_inner(None, false, Vec::new(), None).await
    }

    /// Two-tier scenario boot. Tier-2 (real OpenRouter) runs when
    /// `LETI_LIVE_E2E=1` and a key is present; otherwise tier-1 (the scripted
    /// mock playing `script`). The SAME scenario body (drive + on-disk assert)
    /// runs on both tiers; only the LLM backend differs. `script` is the tier-1
    /// tool/text turn sequence.
    pub async fn for_scenario(script: Vec<ScriptedTurn>) -> Self {
        Self::with_openrouter_inner(None, false, script, None).await
    }

    /// `for_scenario` with the REAL subagent spawner wired in (for the subagent
    /// scenario: tier-2 lets a live model decide to spawn; tier-1 scripts the
    /// `subagent_task` call).
    pub async fn for_scenario_with_subagents(script: Vec<ScriptedTurn>) -> Self {
        Self::with_openrouter_inner(None, true, script, None).await
    }

    /// `for_scenario` with a small compaction window (compaction-continuity
    /// scenario). Tier-1 still exercises the wiring; tier-2 proves a real model
    /// recalls across a real summarization turn. `fallback` is the text served
    /// once the scripted queue drains — compaction inserts extra summarization
    /// turns, so the fallback carries the scenario's sentinel to keep the run
    /// (and the recall) coherent past the scripted turns.
    pub async fn for_scenario_small_window(
        window: u32,
        script: Vec<ScriptedTurn>,
        fallback: Option<String>,
    ) -> Self {
        Self::with_openrouter_inner(Some(window), false, script, fallback).await
    }

    /// Boot against real OpenRouter with a deliberately small agent context
    /// window so a multi-turn conversation crosses the compaction threshold.
    /// Registers a `general` agent (the harness default slug) carrying the
    /// given `context_window`; without a registered agent, `loop_ctx.agent`
    /// is `None` and compaction never fires. Used by the compaction-continuity
    /// live test.
    pub async fn with_openrouter_small_window(context_window: u32) -> Self {
        Self::with_openrouter_inner(Some(context_window), false, Vec::new(), None).await
    }

    /// Boot against real OpenRouter with the REAL subagent spawner wired in
    /// (late-bound to AppState, mirroring main.rs), so a live model's
    /// `subagent_task` call drives an actual child run_loop. Used by the
    /// subagent live test.
    pub async fn with_openrouter_subagents() -> Self {
        Self::with_openrouter_inner(None, true, Vec::new(), None).await
    }

    async fn with_openrouter_inner(
        compaction_window: Option<u32>,
        enable_subagents: bool,
        scripted_turns: Vec<ScriptedTurn>,
        scripted_fallback: Option<String>,
    ) -> Self {
        // On the live tier the key is REQUIRED; on the mock tier (tier-1) the
        // key is absent, so fall back to a placeholder (the scripted provider
        // ignores it).
        let key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "mock-key".to_string());
        let model = std::env::var("LETI_LIVE_E2E_MODEL")
            .unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
        Self::boot(
            leti_adapters::openrouter::DEFAULT_BASE_URL,
            Some(SecretString::from(key)),
            &model,
            None,
            compaction_window,
            enable_subagents,
            scripted_turns,
            true,
            scripted_fallback,
        )
        .await
    }

    /// `data_dir`: `Some` → on-disk sqlite reused across boots (persistence
    /// test); `None` → ephemeral tempdir + in-memory sqlite (fast default).
    #[allow(clippy::too_many_arguments)]
    async fn boot(
        base_url: &str,
        api_key: Option<SecretString>,
        model: &str,
        data_dir: Option<PathBuf>,
        compaction_window: Option<u32>,
        enable_subagents: bool,
        scripted_turns: Vec<ScriptedTurn>,
        tier_scenario: bool,
        scripted_fallback: Option<String>,
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
        let memory: Arc<dyn leti_core::adapters::MemoryStore> =
            Arc::new(SqliteMemoryStore::new(pool.clone()));
        let events: Arc<dyn leti_core::adapters::EventSink> =
            Arc::new(BroadcastBus::with_repo(event_repo));

        // Tier switch. `tier_scenario` distinguishes the two-tier scenario
        // boots (for_scenario*/with_openrouter*) from the `with_mock` path
        // (which always uses OpenRouterProvider pointed at a MockOpenAiService
        // URL). For scenario boots: live (key + flag) → REAL OpenRouter; else
        // the scripted mock driving the SAME body.
        let provider: Arc<dyn ModelProvider> = if !tier_scenario || scenario_live_enabled() {
            Arc::new(OpenRouterProvider::new(
                base_url.to_string(),
                api_key,
                leti_adapters::openrouter::OpenRouterConfig::default(),
            ))
        } else {
            Arc::new(ScriptedProvider {
                turns: Mutex::new(scripted_turns.into()),
                fallback: scripted_fallback.clone(),
            })
        };

        let config = Config {
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: data_root.clone(),
            default_model: model.to_string(),
            permission_ruleset_path: None,
            log_format: LogFormat::Pretty,
            plugins: PluginsConfig::default(),
            tool_scheduler: Default::default(),
        };

        let runtime = Arc::new(ConversationRuntime::new(
            provider.clone(),
            memory.clone(),
            events.clone(),
            RuntimeConfig::new(model.to_string()),
        ));

        let shell_exec = Arc::new(LocalShellExecutor::new(workspace_root.clone()));
        let fs_adapter = Arc::new(LocalFilesystem::new(workspace_root.clone()));
        let shell: Arc<dyn leti_core::tools::builtins::bash::ShellExecutor> = shell_exec.clone();

        let core_api: Arc<dyn CoreApi> = Arc::new(NoopCoreApi);
        let task_registry = Arc::new(leti_core::runtime::subagent::TaskRegistry::new(32));
        // When subagents are enabled, register the REAL spawner (late-bound to
        // AppState below, mirroring main.rs boot order) so a live model's
        // `subagent_task` call actually drives a child run_loop. Otherwise the
        // stub returns Err (the default for tests that don't exercise spawning).
        let real_spawner = if enable_subagents {
            Some(Arc::new(leti_server::RuntimeSubagentSpawner::new()))
        } else {
            None
        };
        let spawner: Arc<dyn leti_core::tools::builtins::subagent_task::SubagentSpawner> =
            match &real_spawner {
                Some(s) => s.clone(),
                None => Arc::new(StubSubagentSpawner),
            };
        // Wire the in-process Monty python executor exactly like the server
        // binary (main.rs passes `Some(python)`), so scenarios can drive the
        // real `python` tool. Harmless for scenarios that never call it — it
        // only adds the tool to the catalog.
        let python: Arc<dyn leti_core::tools::builtins::python::PythonExecutor> =
            Arc::new(leti_adapters::pyexec::MontyExecutor::new());
        let plugins = leti_plugin_registry::all_plugins(
            shell.clone(),
            Some(python),
            None,
            memory.clone(),
            task_registry.clone(),
            spawner,
        );
        let configs = HashMap::new();
        let installed = install_all(plugins, &configs, core_api)
            .await
            .expect("install plugins");

        let mut tool_builder = leti_core::tools::ToolRegistry::builder();
        for tool in installed.tools {
            tool_builder = tool_builder.register_erased(tool);
        }
        let tool_registry = tool_builder.build();

        // Populate the plugin registry the same way the binary does so
        // `GET /v1/plugin*` serves the real registered set, not an empty one.
        let mut plugin_registry = PluginHandles::new();
        for plugin in installed.plugins {
            plugin_registry.push(plugin);
        }

        let default_agent_id = AgentId::new();
        let agent_spec = AgentSpec::new(default_agent_id, workspace_root.clone(), "default");
        let mut agents: HashMap<AgentId, leti_server::AgentResources> = HashMap::new();
        agents.insert(
            default_agent_id,
            leti_server::AgentResources {
                spec: agent_spec,
                fs: fs_adapter.clone(),
                shell: shell.clone(),
            },
        );

        // Register a `general` agent (the harness default session slug) when a
        // caller needs it: a small compaction window (compaction-continuity
        // test) OR subagents enabled (spawn admission resolves `general`).
        // Without a registered agent, `loop_ctx.agent` is `None` (compaction
        // never fires) and `subagent_type: "general"` fails to resolve.
        let mut agent_registry = leti_core::agent::AgentRegistry::new();
        if compaction_window.is_some() || enable_subagents {
            use leti_core::agent::{AgentDefinition, AgentSlug, PromptSegments};
            // Small window only for the compaction test; otherwise a roomy
            // window so subagent turns don't trip compaction mid-flight.
            let window = compaction_window.unwrap_or(128_000);
            let def = AgentDefinition {
                slug: AgentSlug::new("general").expect("slug"),
                title: "General".into(),
                description: String::new(),
                prompt_segments: Some(PromptSegments::default()),
                tool_allowlist: Vec::new(),
                model_id: Some(model.to_string()),
                default_temperature: 0.0,
                context_window: window,
                compaction_threshold: 0.5,
                compaction_summary_cap_tokens: 500,
                hidden: false,
            };
            def.validate().expect("valid compaction agent");
            agent_registry.insert(def).expect("insert general agent");
        }

        let artifacts: Arc<dyn leti_core::adapters::ArtifactStore> =
            Arc::new(LocalFsArtifactStore::new(artifact_root, pool.clone()));
        let state = leti_server::AppStateBuilder::new()
            .provider(provider)
            .memory(memory.clone())
            .artifacts(artifacts.clone())
            .tool_registry(tool_registry)
            .events(events)
            .permission(Arc::new(ConfigPermissionMgr::new()))
            .config(Arc::new(config))
            .plugin_registry(Arc::new(plugin_registry))
            .runtime(runtime)
            .agents(agents)
            .default_agent_id(default_agent_id)
            .workspace_root(workspace_root.clone())
            .agent_registry(Arc::new(agent_registry))
            .build()
            .expect("build app state");

        // Late-bind the live AppState into the real subagent spawner, the same
        // boot order as main.rs (spawner built before plugins, bound after the
        // state exists). Only when subagents are enabled for this boot.
        if let Some(s) = &real_spawner {
            s.set_state(state.clone());
        }

        // Mirror the binary (`main.rs`): the question/answer route hard-requires
        // an `AuthPrincipal` extension. Without this layer a real model's
        // `ask_user` answer POST → 401 → the registry oneshot never resolves →
        // the runner hangs to its 300s timeout. Inject it like the binary does.
        let app = leti_server::build_router(state)
            .layer(axum::Extension(leti_server::AuthPrincipal::user("test")));
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
            default_agent_id,
            artifacts,
            serve_task,
            _tempdir: owned_tempdir,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Create a session with `user_questions` capability ENABLED, persisted
    /// directly via the same memory store the server uses. The default
    /// `POST /v1/session` route hardcodes capabilities to `{}` (headless-safe),
    /// so `ask_user` would error synchronously; this opt-in path lets the
    /// ask_user live test exercise the real park→answer→resume flow.
    ///
    /// Built by reading a normally-created session's meta (so the private
    /// `permission_mode` default is inherited, not named here), re-stamping a
    /// fresh id, flipping the capability on, and re-persisting.
    pub async fn create_question_capable_session(&self) -> String {
        let seed = self
            .memory
            .create_session(self.default_agent_id, None)
            .await
            .expect("seed session");
        let mut meta = self
            .memory
            .get_session(seed)
            .await
            .expect("get seed meta")
            .expect("seed meta present");
        meta.id = leti_core::types::session::SessionId::new();
        meta.capabilities.user_questions = true;
        meta.model = Some(self.model.clone());
        self.memory
            .create_session_with_meta(meta.clone())
            .await
            .expect("create question-capable session");
        meta.id.to_string()
    }

    /// `POST /v1/session/:id/question/answer` — resolve a pending `ask_user`
    /// question by id with the chosen option indices. Returns the status code
    /// so the caller can assert acceptance (200) vs not-found (404).
    pub async fn answer_question(
        &self,
        session_id: &str,
        question_id: &str,
        selected: Vec<usize>,
    ) -> reqwest::StatusCode {
        self.http
            .post(format!(
                "{}/v1/session/{session_id}/question/answer",
                self.base_url
            ))
            .json(&serde_json::json!({
                "question_id": question_id,
                "selected": selected,
            }))
            .send()
            .await
            .expect("answer question")
            .status()
    }

    /// Read a persisted artifact by key from the same store the server uses,
    /// returning its bytes (or `None` if absent). The local store resolves by
    /// `(session, key)` and ignores the ref's `size`, so a minimal ref works.
    /// Used by the todo workflow test to read back `todos.json`.
    pub async fn read_artifact(&self, session_id: &str, key: &str) -> Option<Vec<u8>> {
        use leti_core::adapters::artifact_store::ArtifactRef;
        let session = leti_core::types::session::SessionId(session_id.parse().expect("uuid"));
        let r = ArtifactRef {
            session_id: session,
            key: key.to_string(),
            size: 0,
            mime: None,
        };
        self.artifacts.get(&r).await.ok().map(|b| b.to_vec())
    }

    /// Open the session SSE stream and read frames until a `question.requested`
    /// event arrives, returning its `question_id`. Unlike
    /// `collect_session_events`, this returns BEFORE terminal status — the turn
    /// is parked on the pending question and will not reach terminal until the
    /// caller answers it. Returns `None` if the deadline elapses first.
    pub async fn wait_for_question(&self, session_id: &str, deadline: Duration) -> Option<String> {
        use futures::StreamExt as _;

        let url = format!("{}/v1/event?session={session_id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("accept", "text/event-stream")
            .send()
            .await
            .expect("sse connect");
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();

        let find = async {
            while let Some(chunk) = stream.next().await {
                let Ok(bytes) = chunk else { break };
                buf.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(idx) = buf.find("\n\n") {
                    let raw = buf[..idx].to_string();
                    buf.drain(..idx + 2);
                    if let Some(data) = parse_sse_data(&raw)
                        && let Ok(json) = serde_json::from_str::<Value>(&data)
                        && json.get("kind").and_then(Value::as_str) == Some("question_requested")
                        && let Some(qid) = json.get("question_id").and_then(Value::as_str)
                    {
                        return Some(qid.to_string());
                    }
                }
            }
            None
        };

        tokio::time::timeout(deadline, find).await.ok().flatten()
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
    pub fn memory(&self) -> &Arc<dyn leti_core::adapters::MemoryStore> {
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
                    if let Some(data) = parse_sse_data(&raw)
                        && let Ok(json) = serde_json::from_str::<Value>(&data)
                    {
                        let terminal = is_terminal_status(&json);
                        frames.push(json);
                        if terminal {
                            return;
                        }
                    }
                }
            }
        };

        let _ = tokio::time::timeout(deadline, collect).await;
        frames
    }
}

/// Tier switch for the shared scenario harness: tier-2 (real OpenRouter) when
/// `LETI_LIVE_E2E=1` AND a key is present; otherwise tier-1 (scripted mock).
/// Centralized so every scenario file branches identically.
pub fn scenario_live_enabled() -> bool {
    std::env::var("LETI_LIVE_E2E").as_deref() == Ok("1") && std::env::var("OPENAI_API_KEY").is_ok()
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

// --- Scripted mock provider (tier-1 backend) -------------------------------
//
// Emits a pre-scripted sequence of turns so a tier-1 (no-key) run drives the
// SAME scenario body as the tier-2 (live OpenRouter) run. Each "turn" is the
// delta list the runtime sees for one model step: a text turn ends the loop,
// a tool turn makes the runtime dispatch a real builtin tool (real fs/shell/
// memory) and feed the result back into the NEXT scripted turn. The mock fakes
// only the LLM's decisions, never the tool execution — so a tier-1 pass proves
// the same wiring a tier-2 pass exercises.

use leti_core::adapters::model_provider::{
    ChatDelta, ChatRequest, ChatStream, FinishReason, ModelInfo, ModelPricing,
};
use leti_core::error::ProviderError;
use leti_core::types::event::Usage;
use tokio_util::sync::CancellationToken;

/// One model step's worth of deltas.
pub type ScriptedTurn = Vec<Result<ChatDelta, ProviderError>>;

/// A text turn that ends the loop. Carries usage so the cost path is non-zero.
pub fn text_turn(text: &str) -> ScriptedTurn {
    vec![
        Ok(ChatDelta::Role),
        Ok(ChatDelta::Content { text: text.into() }),
        Ok(ChatDelta::Finish {
            reason: FinishReason::EndTurn,
            usage: Some(Usage {
                input_tokens: 1000,
                output_tokens: 100,
                ..Default::default()
            }),
        }),
    ]
}

/// A tool-call turn: the runtime dispatches `name` with `args_json` (a real
/// builtin against real adapters), then continues with the next scripted turn.
pub fn tool_turn(call_id: &str, name: &str, args_json: &str) -> ScriptedTurn {
    vec![
        Ok(ChatDelta::Role),
        Ok(ChatDelta::ToolCallStart {
            call_id: call_id.into(),
            name: name.into(),
            index: 0,
        }),
        Ok(ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: args_json.into(),
        }),
        Ok(ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: Some(Usage {
                input_tokens: 1000,
                output_tokens: 50,
                ..Default::default()
            }),
        }),
    ]
}

/// Pops one scripted turn per `chat_stream`; peeks the cancel token between
/// deltas so a tripped token yields a synthetic `Cancelled` finish.
struct ScriptedProvider {
    turns: Mutex<VecDeque<ScriptedTurn>>,
    /// Text for a synthetic turn served once the queue drains (compaction
    /// over-run guard). `None` → an empty stream when exhausted.
    fallback: Option<String>,
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    async fn chat_stream(
        &self,
        _req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        // Pop the next scripted turn. When the queue drains, emit a fresh text
        // turn built from `fallback` (if set) — compaction inserts extra
        // summarization turns unpredictably, and an over-run must not strand the
        // run on an empty stream. The fallback carries the scenario's sentinel
        // (e.g. the recalled fact) so every served turn preserves it.
        let deltas = {
            let popped = self.turns.lock().unwrap().pop_front();
            match popped {
                Some(t) => t,
                None => match &self.fallback {
                    Some(text) => text_turn(text),
                    None => Vec::new(),
                },
            }
        };
        let stream = stream::unfold(
            (deltas.into_iter(), cancel, false),
            |(mut iter, cancel, done)| async move {
                if done {
                    return None;
                }
                if cancel.is_cancelled() {
                    let frame = Ok(ChatDelta::Finish {
                        reason: FinishReason::Cancelled,
                        usage: None,
                    });
                    return Some((frame, (iter, cancel, true)));
                }
                iter.next().map(|d| (d, (iter, cancel, done)))
            },
        );
        Ok(Box::new(stream.boxed()) as ChatStream)
    }

    fn pricing(&self, _model: &str) -> Option<ModelPricing> {
        Some(ModelPricing {
            input_per_mtok: Decimal::ONE,
            output_per_mtok: Decimal::ONE,
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
        })
    }

    /// A small fixed catalog so the tier-1 models test exercises the
    /// `GET /v1/models` route serialization (real plumbing) — distinct from the
    /// trait default (`[]`). The live tier returns the real OpenRouter catalog.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![
            ModelInfo {
                id: "mock/model-small".into(),
                display_name: Some("Mock Small".into()),
                context_length: Some(8192),
            },
            ModelInfo {
                id: "mock/model-large".into(),
                display_name: Some("Mock Large".into()),
                context_length: Some(128_000),
            },
        ])
    }
}

// --- Inline stubs (mirrors support.rs; kept local so this harness is
// self-contained and doesn't couple to the oneshot harness module). ---

struct NoopCoreApi;

#[async_trait]
impl CoreApi for NoopCoreApi {
    async fn current_session_meta(
        &self,
        _: leti_core::types::session::SessionId,
    ) -> Option<leti_core::types::session::SessionMeta> {
        None
    }
    fn session_cost(&self, _: leti_core::types::session::SessionId) -> Decimal {
        Decimal::ZERO
    }
    fn record_cost(&self, _: leti_core::types::session::SessionId, _: Decimal) {}
    async fn emit_event(
        &self,
        _: leti_core::types::event::AgentEvent,
        _: leti_core::adapters::event_sink::Persistence,
    ) {
    }
    fn read_config(&self, _: &str) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn cancel_session(&self, _: leti_core::types::session::SessionId, _: String) {}
    async fn emit_notification(
        &self,
        _: Option<leti_core::types::session::SessionId>,
        _: leti_core::hooks::io::NotificationLevel,
        _: String,
        _: String,
        _: String,
    ) {
    }
}

/// Build a minimal `ToolCtx` for running an executor (bash/python) directly
/// from a test, outside the HTTP loop. Only `fs` + `cancel` carry real
/// behavior — everything else is an all-permissive no-op — because the
/// executors touch nothing else. Used by the debug→fix→verify test to re-run
/// the model's repaired script through a FRESH executor (independent proof the
/// fix actually executes, not just that the file changed).
pub fn minimal_tool_ctx(
    fs: Arc<dyn leti_core::adapters::Filesystem>,
) -> leti_core::adapters::tool_executor::ToolCtx {
    use leti_core::adapters::tool_executor::ToolCtx;
    use leti_core::tools::ReadHistory;
    use leti_core::types::agent::AgentId;
    use leti_core::types::message::MessageId;
    use leti_core::types::permission::PermissionMode;
    use leti_core::types::session::SessionId;

    ToolCtx {
        ext: Default::default(),
        session_id: SessionId::new(),
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "verify-rerun".into(),
        fs,
        mode: PermissionMode::Danger,
        permission: Arc::new(AllowAllPerm),
        events: Arc::new(NoopEventSink),
        artifacts: Arc::new(DiscardArtifacts),
        read_history: ReadHistory::new(),
        cancel: CancellationToken::new(),
        questions: Arc::new(leti_core::runtime::QuestionRegistry::new()),
        memory: Arc::new(NoopMemory),
        task_registry: Arc::new(leti_core::runtime::subagent::TaskRegistry::new(32)),
        agent_registry: Arc::new(leti_core::agent::AgentRegistry::new()),
    }
}

struct AllowAllPerm;

#[async_trait]
impl leti_core::adapters::permission_manager::PermissionManager for AllowAllPerm {
    async fn check(
        &self,
        _: leti_core::types::permission::PermissionCtx,
        _: leti_core::types::permission::PermissionRequest,
    ) -> Result<leti_core::types::permission::Decision, leti_core::error::PermissionError> {
        Ok(leti_core::types::permission::Decision::Allow)
    }
    async fn reply(
        &self,
        _: leti_core::types::permission::AskId,
        _: leti_core::types::permission::Decision,
    ) -> Result<(), leti_core::error::PermissionError> {
        Ok(())
    }
    async fn cancel_ask(
        &self,
        _: leti_core::types::permission::AskId,
    ) -> Result<(), leti_core::error::PermissionError> {
        Ok(())
    }
    async fn record_always(
        &self,
        _: leti_core::types::permission::AlwaysScope,
        _: leti_core::types::permission::PermissionRule,
    ) -> Result<(), leti_core::error::PermissionError> {
        Ok(())
    }
    fn take_deferred(
        &self,
        _: leti_core::types::permission::AskId,
    ) -> Option<leti_core::permission::Deferred<leti_core::types::permission::Decision>> {
        None
    }
    fn peek_session_id(
        &self,
        _: leti_core::types::permission::AskId,
    ) -> Option<leti_core::types::session::SessionId> {
        None
    }
    async fn accept_ask(
        &self,
        _: leti_core::types::permission::AskId,
        _: leti_core::types::permission::AlwaysScope,
        _: leti_core::types::permission::PermissionAction,
    ) -> Result<(), leti_core::error::PermissionError> {
        Ok(())
    }
}

struct NoopEventSink;

#[async_trait]
impl leti_core::adapters::event_sink::EventSink for NoopEventSink {
    async fn publish(
        &self,
        _: leti_core::types::event::AgentEvent,
        _: leti_core::adapters::event_sink::Persistence,
    ) -> Result<(), leti_core::error::EventError> {
        Ok(())
    }
    fn subscribe(
        &self,
        _: leti_core::types::event::EventFilter,
    ) -> tokio::sync::broadcast::Receiver<leti_core::adapters::event_sink::DeliveredEvent> {
        let (_, rx) = tokio::sync::broadcast::channel(1);
        rx
    }
}

struct DiscardArtifacts;

#[async_trait]
impl leti_core::adapters::artifact_store::ArtifactStore for DiscardArtifacts {
    async fn put(
        &self,
        session: leti_core::types::session::SessionId,
        key: &str,
        _: bytes::Bytes,
    ) -> Result<leti_core::adapters::artifact_store::ArtifactRef, leti_core::error::ArtifactError>
    {
        Ok(leti_core::adapters::artifact_store::ArtifactRef {
            session_id: session,
            key: key.to_string(),
            size: 0,
            mime: None,
        })
    }
    async fn get(
        &self,
        _: &leti_core::adapters::artifact_store::ArtifactRef,
    ) -> Result<bytes::Bytes, leti_core::error::ArtifactError> {
        Err(leti_core::error::ArtifactError::NotFound("test".into()))
    }
    async fn list(
        &self,
        _: leti_core::types::session::SessionId,
    ) -> Result<
        Vec<leti_core::adapters::artifact_store::ArtifactRef>,
        leti_core::error::ArtifactError,
    > {
        Ok(vec![])
    }
}

struct NoopMemory;

#[async_trait]
impl leti_core::adapters::memory_store::MemoryStore for NoopMemory {
    async fn create_session(
        &self,
        _: leti_core::types::agent::AgentId,
        _: Option<leti_core::types::session::SessionId>,
    ) -> Result<leti_core::types::session::SessionId, leti_core::error::MemoryError> {
        Err(leti_core::error::MemoryError::Unimplemented)
    }
    async fn get_session(
        &self,
        _: leti_core::types::session::SessionId,
    ) -> Result<Option<leti_core::types::session::SessionMeta>, leti_core::error::MemoryError> {
        Ok(None)
    }
    async fn list_sessions(
        &self,
        _: leti_core::types::session::SessionFilter,
    ) -> Result<Vec<leti_core::types::session::SessionMeta>, leti_core::error::MemoryError> {
        Ok(vec![])
    }
    async fn update_status(
        &self,
        _: leti_core::types::session::SessionId,
        _: leti_core::types::session::SessionStatus,
        _: &str,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn switch_agent(
        &self,
        _: leti_core::types::session::SessionId,
        _: &str,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn update_permission_mode(
        &self,
        _: leti_core::types::session::SessionId,
        _: leti_core::types::permission::PermissionMode,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn update_session_extensions(
        &self,
        _: leti_core::types::session::SessionId,
        _: Value,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn delete_session(
        &self,
        _: leti_core::types::session::SessionId,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn append_message(
        &self,
        _: leti_core::types::session::SessionId,
        msg: leti_core::types::message::Message,
    ) -> Result<leti_core::types::message::MessageId, leti_core::error::MemoryError> {
        Ok(msg.id)
    }
    async fn append_part(
        &self,
        _: leti_core::types::message::MessageId,
        _: leti_core::types::part::Part,
    ) -> Result<leti_core::types::part::PartId, leti_core::error::MemoryError> {
        Ok(leti_core::types::part::PartId::new())
    }
    async fn upsert_part(
        &self,
        _: leti_core::types::message::MessageId,
        _: leti_core::types::part::PartId,
        _: leti_core::types::part::Part,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn list_messages(
        &self,
        _: leti_core::types::session::SessionId,
    ) -> Result<Vec<leti_core::types::message::Message>, leti_core::error::MemoryError> {
        Ok(vec![])
    }
    async fn list_parts(
        &self,
        _: leti_core::types::session::SessionId,
        _: leti_core::types::message::MessageId,
    ) -> Result<Vec<leti_core::types::part::Part>, leti_core::error::MemoryError> {
        Ok(vec![])
    }
    async fn record_read(
        &self,
        _: leti_core::types::session::SessionId,
        _: std::path::PathBuf,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
}

struct StubSubagentSpawner;

#[async_trait]
impl leti_core::tools::builtins::subagent_task::SubagentSpawner for StubSubagentSpawner {
    async fn spawn(
        &self,
        _ctx: &leti_core::adapters::tool_executor::ToolCtx,
        _subagent_type: &str,
        _objective: &str,
        _scope: Option<&str>,
        _background: bool,
    ) -> Result<
        leti_core::tools::builtins::subagent_task::SpawnedSubagent,
        leti_core::runtime::subagent::SpawnError,
    > {
        Err(leti_core::runtime::subagent::SpawnError::Internal(
            "stub spawner".into(),
        ))
    }
    async fn await_completion(
        &self,
        _task_id: leti_core::runtime::subagent::TaskId,
    ) -> Result<
        (
            String,
            Option<String>,
            leti_core::runtime::subagent::TaskStatus,
        ),
        leti_core::runtime::subagent::SpawnError,
    > {
        Err(leti_core::runtime::subagent::SpawnError::Internal(
            "stub spawner".into(),
        ))
    }
}
