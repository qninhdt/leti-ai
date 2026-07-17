//! Bounded, process-wide admission and keyed resource locking for tools.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Weak};

use dashmap::DashMap;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingMode {
    Concurrent,
    Exclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceAccess {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceKey {
    Workspace,
    WorkspacePath(PathBuf),
    Session(String),
    Task(String),
    Custom { namespace: String, key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceClaim {
    pub key: ResourceKey,
    pub access: ResourceAccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolConcurrency {
    pub mode: SchedulingMode,
    pub claims: Vec<ResourceClaim>,
}

impl ToolConcurrency {
    #[must_use]
    pub fn concurrent() -> Self {
        Self {
            mode: SchedulingMode::Concurrent,
            claims: vec![],
        }
    }
    #[must_use]
    pub fn exclusive() -> Self {
        Self {
            mode: SchedulingMode::Exclusive,
            claims: vec![],
        }
    }
    #[must_use]
    pub fn with_claim(mut self, key: ResourceKey, access: ResourceAccess) -> Self {
        self.claims.push(ResourceClaim { key, access });
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ToolSchedulerConfig {
    pub max_per_turn: usize,
    pub max_global: usize,
}
impl Default for ToolSchedulerConfig {
    fn default() -> Self {
        Self {
            max_per_turn: 8,
            max_global: 64,
        }
    }
}
impl ToolSchedulerConfig {
    pub fn validate(self) -> Result<Self, String> {
        if self.max_per_turn == 0 || self.max_global == 0 {
            return Err("tool scheduler limits must be positive".into());
        }
        if self.max_per_turn > self.max_global {
            return Err("LETI_TOOL_MAX_PER_TURN must not exceed LETI_TOOL_MAX_GLOBAL".into());
        }
        Ok(self)
    }
}

/// Shared per-process scheduler. Weak entries allow locks for stale keys to
/// disappear naturally once no invocation holds one.
pub struct ToolScheduler {
    global: Arc<Semaphore>,
    locks: DashMap<String, Weak<RwLock<()>>>,
    config: ToolSchedulerConfig,
}
impl ToolScheduler {
    #[must_use]
    pub fn new(config: ToolSchedulerConfig) -> Self {
        let config = config.validate().expect("validated tool scheduler config");
        Self {
            global: Arc::new(Semaphore::new(config.max_global)),
            locks: DashMap::new(),
            config,
        }
    }
    #[must_use]
    pub fn config(&self) -> ToolSchedulerConfig {
        self.config
    }
    #[must_use]
    pub fn turn_semaphore(&self) -> Arc<Semaphore> {
        Arc::new(Semaphore::new(self.config.max_per_turn))
    }
    fn lock(&self, key: &str) -> Arc<RwLock<()>> {
        if let Some(existing) = self.locks.get(key).and_then(|v| v.upgrade()) {
            return existing;
        }
        let lock = Arc::new(RwLock::new(()));
        self.locks.insert(key.to_owned(), Arc::downgrade(&lock));
        lock
    }
    pub async fn acquire(
        &self,
        turn: Arc<Semaphore>,
        claims: Vec<ResourceClaim>,
        cancel: &CancellationToken,
    ) -> Result<Admission, ()> {
        let turn = tokio::select! { biased; p = turn.acquire_owned() => p.expect("turn semaphore open"), () = cancel.cancelled() => return Err(()) };
        let mut guards = Vec::new();
        for (key, access) in normalize_claims(claims) {
            let lock = self.lock(&key);
            let guard = match access {
                ResourceAccess::Read => {
                    tokio::select! { biased; g = lock.read_owned() => HeldLock::Read(g), () = cancel.cancelled() => return Err(()) }
                }
                ResourceAccess::Write => {
                    tokio::select! { biased; g = lock.write_owned() => HeldLock::Write(g), () = cancel.cancelled() => return Err(()) }
                }
            };
            guards.push(guard);
        }
        let global = tokio::select! { biased; p = self.global.clone().acquire_owned() => p.expect("global semaphore open"), () = cancel.cancelled() => return Err(()) };
        Ok(Admission {
            _turn: turn,
            _global: global,
            _locks: guards,
        })
    }
}
#[allow(dead_code)] // the guards are intentionally retained for RAII release.
enum HeldLock {
    Read(OwnedRwLockReadGuard<()>),
    Write(OwnedRwLockWriteGuard<()>),
}
pub struct Admission {
    _turn: tokio::sync::OwnedSemaphorePermit,
    _global: tokio::sync::OwnedSemaphorePermit,
    _locks: Vec<HeldLock>,
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s),
            Component::RootDir | Component::Prefix(_) => out.push(c.as_os_str()),
        }
    }
    out
}
pub fn normalize_claims(claims: Vec<ResourceClaim>) -> Vec<(String, ResourceAccess)> {
    let mut out = BTreeMap::new();
    for claim in claims {
        let key = match claim.key {
            ResourceKey::Workspace => "workspace".into(),
            ResourceKey::WorkspacePath(p) => {
                format!("workspace-path:{}", normalize_path(&p).display())
            }
            ResourceKey::Session(s) => format!("session:{s}"),
            ResourceKey::Task(s) => format!("task:{s}"),
            ResourceKey::Custom { namespace, key } => format!("custom:{namespace}:{key}"),
        };
        out.entry(key)
            .and_modify(|a| {
                if claim.access == ResourceAccess::Write {
                    *a = ResourceAccess::Write;
                }
            })
            .or_insert(claim.access);
    }
    out.into_iter().collect()
}
