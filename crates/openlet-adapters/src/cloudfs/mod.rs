//! `CloudFilesystem` — `Filesystem` impl backed by openlet file-service gRPC.
//!
//! Local vs cloud differ ONLY in the injected `Filesystem` impl; the emulated
//! bash/python interpreters are identical (they only ever hold `Arc<dyn
//! Filesystem>`). This impl maps each trait method onto the file-service
//! backend (Postgres + S3) verified in openlet:
//!
//! | trait method   | backend                                                    |
//! |----------------|------------------------------------------------------------|
//! | `grep`         | 2-phase: `GrepFiles` trigram literal prefilter → in-proc   |
//! |                | linear-time `regex` re-match (ReDoS-safe, dialect parity)  |
//! | `glob`/`list`  | folder-tree walk via `ListFolders`/`ListFiles`             |
//! | `read`/`stat`  | `GetFile(include_text)` — `extracted_text` column          |
//! | `exists`       | path→id resolve + `GetFile`                                |
//! | `remove`       | path→id resolve + `DeleteFile`                             |
//! | `rename`       | path→id resolve + `PatchFile` (rename) / `MoveFile` (move) |
//! | `write`/`append`| NOT wired this phase — returns `FsError::Unsupported`     |
//!
//! ## Scope note (Phase 6)
//! The read path is complete; the mutation path is intentionally partial.
//! `write`/`append` require the presigned-PUT upload dance (`CreateUploadIntent`
//! → S3 PUT → `CompleteUpload`) plus an HTTP client openlet-ai does not yet
//! carry, and neither has any coverage in this phase's success criteria. They
//! return `FsError::Unsupported` (→ `ToolError::Unimplemented`) so a cloud-mode
//! agent gets a clean "not supported here" rather than a silent wrong answer.
//!
//! ## Auth
//! Every RPC carries the caller's bearer JWT in the `authorization` gRPC
//! metadata; file-service resolves workspace membership from it (same gate as
//! its HTTP surface). The workspace is fixed at construction.
//!
//! ## Deploy-ordering contract
//! `grep` calls the `GrepFiles` RPC, which only exists after file-service ships
//! migration 000016 + the GrepFiles handler. Against an older backend the call
//! returns gRPC `Unimplemented`, surfaced here as `FsError::Io`. Cloud mode
//! must stay flag-OFF until the backend is deployed (see plan Phase 6).

mod convert;
mod literals;
mod rematch;
mod resolve;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use openlet_core::adapters::filesystem::{
    ByteRange, DirEntry, FileMeta, Filesystem, GlobOpts, GrepArgs, GrepHit, WriteOpts,
};
use openlet_core::error::FsError;
use tonic::Request;
use tonic::transport::Channel;

use self::convert::{file_info_to_meta, normalize_rel, status_to_fs};
use self::rematch::Candidate;

/// Generated tonic client + prost types for file-service. The module name is
/// the proto package (`openlet.file.v1`).
#[allow(clippy::all, clippy::pedantic, missing_docs)]
pub mod pb {
    tonic::include_proto!("openlet.file.v1");
}

use pb::file_service_client::FileServiceClient;

/// Default per-call grep candidate ceiling requested from the backend. The
/// backend clamps to its own ceiling regardless; this keeps the fetched
/// payload bounded for the in-process re-match.
const GREP_BACKEND_MAX_HITS: i32 = 2000;

/// Page size for `ListFolders`/`ListFiles`. Paired with token-following in
/// `list_all_folders`/`list_all_files` so a directory with more children than
/// this is fully enumerated rather than silently truncated.
const PAGE_SIZE: i32 = 1000;

/// Hard ceiling on pages followed per listing, so a misbehaving backend that
/// keeps returning a non-empty `next_page_token` cannot loop forever.
const MAX_PAGES: usize = 10_000;

/// `Filesystem` impl over file-service gRPC.
///
/// Cloneable channel is cheap (tonic multiplexes over one HTTP/2 connection).
/// The session-dirty set is shared so union scans see every write in the
/// session (see [`CloudFilesystem::grep`]).
pub struct CloudFilesystem {
    channel: Channel,
    workspace_id: String,
    bearer: String,
    /// Paths written/appended during this session whose bytes may not be
    /// indexed yet (async Kafka). `grep` unions an in-process scan over these
    /// so a just-written file is grep-visible before the index catches up
    /// (read-after-write parity vs `LocalFilesystem`).
    ///
    /// Deliberately NOT populated by `rename` (reviewer M3): `PatchFile`/
    /// `MoveFile` mutate Postgres synchronously, so a renamed file is already
    /// returned by the backend grep — marking it dirty would double-count it as
    /// a duplicate hit. Only `write`/`append` (async-indexed, stubbed this
    /// phase) belong here, so the set stays empty until those land. The union
    /// machinery is built now so wiring writes later needs no parity rework.
    session_dirty: Mutex<Vec<PathBuf>>,
}

impl CloudFilesystem {
    /// Build a cloud filesystem bound to `workspace_id`, dialing `channel`,
    /// authenticating every call with `bearer` (the full `Bearer <jwt>`
    /// string).
    #[must_use]
    pub fn new(channel: Channel, workspace_id: String, bearer: String) -> Self {
        Self {
            channel,
            workspace_id,
            bearer,
            session_dirty: Mutex::new(Vec::new()),
        }
    }

    fn client(&self) -> FileServiceClient<Channel> {
        FileServiceClient::new(self.channel.clone())
    }

    /// Wrap `msg` in a request carrying the bearer in `authorization` metadata.
    fn authed<T>(&self, msg: T) -> Result<Request<T>, FsError> {
        let mut req = Request::new(msg);
        let val = self
            .bearer
            .parse()
            .map_err(|_| FsError::Io("invalid bearer metadata".into()))?;
        req.metadata_mut().insert("authorization", val);
        Ok(req)
    }

    /// Record a path in the session-dirty set (dedup on insert). Reserved for
    /// the `write`/`append` path (stubbed this phase); see `session_dirty` on
    /// why `rename` must NOT call it.
    #[allow(
        dead_code,
        reason = "wired when write/append land; see session_dirty doc"
    )]
    fn mark_dirty(&self, path: &Path) {
        let mut d = self.session_dirty.lock().unwrap_or_else(|e| e.into_inner());
        let p = path.to_path_buf();
        if !d.contains(&p) {
            d.push(p);
        }
    }

    /// Snapshot the session-dirty paths.
    fn dirty_snapshot(&self) -> Vec<PathBuf> {
        self.session_dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[async_trait]
impl Filesystem for CloudFilesystem {
    async fn read(&self, path: &Path, range: Option<ByteRange>) -> Result<Bytes, FsError> {
        let file_id = self.resolve_file_id(path).await?;
        let info = self
            .client()
            .get_file(self.authed(pb::GetFileRequest {
                file_id,
                include_text: true,
            })?)
            .await
            .map_err(status_to_fs)?
            .into_inner();

        // Cloud read serves the indexed `extracted_text`. This is the (possibly
        // truncated) text body, NOT the raw bytes — full/raw fidelity would need
        // a presigned-GET download RPC file-service does not expose today. That
        // is a documented cloud constraint (plan Phase 6): grep/read see the
        // indexed text; a caller needing exact full bytes of a large file is out
        // of scope for this phase.
        //
        // Fail LOUDLY on a binary file rather than returning empty bytes
        // silently (reviewer M2): a binary file has no `extracted_text`, so a
        // naive read would hand back empty content that looks like a valid empty
        // file. `FsError::Binary` → `ToolError::BinaryFile` tells the caller the
        // truth. (Text files with empty bodies are still legitimately empty.)
        if file_info_to_meta(&info).is_binary {
            return Err(FsError::Binary(normalize_rel(path)?));
        }
        let full = Bytes::from(info.extracted_text.into_bytes());
        match range {
            None => Ok(full),
            Some(r) => {
                let start = usize::try_from(r.start)
                    .unwrap_or(usize::MAX)
                    .min(full.len());
                let end = if r.len == 0 {
                    full.len()
                } else {
                    start
                        .saturating_add(usize::try_from(r.len).unwrap_or(usize::MAX))
                        .min(full.len())
                };
                Ok(full.slice(start..end))
            }
        }
    }

    async fn stat(&self, path: &Path) -> Result<FileMeta, FsError> {
        let file_id = self.resolve_file_id(path).await?;
        let info = self
            .client()
            .get_file(self.authed(pb::GetFileRequest {
                file_id,
                include_text: false,
            })?)
            .await
            .map_err(status_to_fs)?
            .into_inner();
        Ok(file_info_to_meta(&info))
    }

    async fn exists(&self, path: &Path) -> bool {
        // Never errors per the trait contract; any resolution failure is false.
        self.resolve_file_id(path).await.is_ok()
    }

    async fn write(
        &self,
        _path: &Path,
        _body: Bytes,
        _opts: WriteOpts,
    ) -> Result<FileMeta, FsError> {
        // See module scope note: the presigned-PUT upload path is not wired in
        // this phase. Surfaces as ToolError::Unimplemented at the tool boundary.
        Err(FsError::Unsupported(
            "cloud write not supported in this phase (presigned-PUT path unwired)".into(),
        ))
    }

    async fn remove(&self, path: &Path) -> Result<(), FsError> {
        let file_id = self.resolve_file_id(path).await?;
        self.client()
            .delete_file(self.authed(pb::DeleteFileRequest { file_id })?)
            .await
            .map_err(status_to_fs)?;
        Ok(())
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        let file_id = self.resolve_file_id(from).await?;
        let (from_folder, _) = self.resolve_parent_folder(from).await?;
        let (to_folder, to_name) = self.resolve_parent_folder(to).await?;

        // A move (different parent folder) uses MoveFile; a pure rename (same
        // folder, new name) uses PatchFile. A rename that BOTH moves and renames
        // needs both RPCs — move first, then patch the name.
        let moved = from_folder != to_folder;
        if moved {
            let dst = to_folder.clone().ok_or_else(|| {
                // MoveFile cannot express "move to workspace root" over gRPC yet
                // (backend limitation: empty string is ambiguous with absent).
                FsError::Unsupported("cloud move to workspace root not supported over gRPC".into())
            })?;
            self.client()
                .move_file(self.authed(pb::MoveFileRequest {
                    file_id: file_id.clone(),
                    folder_id: dst,
                })?)
                .await
                .map_err(status_to_fs)?;
        }
        // Patch the name whenever the leaf name changes.
        let from_name = normalize_rel(from)?
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        if from_name != to_name {
            self.client()
                .patch_file(self.authed(pb::PatchFileRequest {
                    file_id,
                    name: to_name,
                })?)
                .await
                .map_err(status_to_fs)?;
        }
        // NOTE: deliberately NOT marked session-dirty. `MoveFile`/`PatchFile`
        // mutate Postgres synchronously, so the renamed row is already visible
        // to the backend `GrepFiles`/`ListFiles`. Adding it to the dirty set
        // would make the grep union re-match it a second time → duplicate hits
        // (reviewer M3). `mark_dirty` is reserved for write/append, whose bytes
        // land in S3 and are only indexed asynchronously via Kafka.
        Ok(())
    }

    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        // Resolve `path` to a folder id (empty path = workspace root), then list
        // that folder's immediate child folders + files (both fully paginated).
        let folder_id = self.resolve_folder_id_for_dir(path).await?;
        let folders = self.list_all_folders(folder_id.as_deref()).await?;
        let files = self.list_all_files(folder_id.as_deref()).await?;

        let mut out: Vec<DirEntry> = Vec::with_capacity(folders.len() + files.len());
        for f in folders {
            out.push(DirEntry {
                name: f.name,
                is_dir: true,
                size: None,
            });
        }
        for f in files {
            out.push(DirEntry {
                name: f.name,
                is_dir: false,
                size: Some(u64::try_from(f.size_bytes).unwrap_or(0)),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn glob(&self, pattern: &str, opts: GlobOpts) -> Result<Vec<PathBuf>, FsError> {
        // Walk the folder tree collecting every file path, then match the glob
        // in-process (globset, same matcher as local). Bounded by max_results.
        let matcher = globset::Glob::new(pattern)
            .map_err(|e| FsError::InvalidInput(e.to_string()))?
            .compile_matcher();
        let all = self.walk_all_files().await?;
        let mut hits: Vec<PathBuf> = all
            .into_iter()
            .map(|(p, _)| p)
            .filter(|p| matcher.is_match(p))
            .collect();
        hits.sort();
        hits.truncate(opts.max_results);
        Ok(hits)
    }

    async fn grep(&self, args: GrepArgs) -> Result<Vec<GrepHit>, FsError> {
        // Compile the pattern up front so a bad regex fails identically to the
        // local path before any network work.
        let re = rematch::compile(&args)?;

        // Phase 1: literal prefilter on the backend. The caller regex NEVER
        // reaches Postgres — only literal fragments do (ReDoS-safe).
        let lits = literals::extract_literals(&args.pattern, args.case_insensitive);
        let resp = self
            .client()
            .grep_files(self.authed(pb::GrepFilesRequest {
                workspace_id: self.workspace_id.clone(),
                literals: lits,
                max_hits: GREP_BACKEND_MAX_HITS,
            })?)
            .await
            .map_err(status_to_fs)?
            .into_inner();

        // Build candidate list from backend rows. Path is folder-relative name;
        // we cannot cheaply reconstruct the full folder path per candidate, so
        // we surface the file name the backend returns (parity with the index
        // view). Optional path_glob filters on that name.
        let path_glob = match args.path_glob.as_deref() {
            Some(p) => Some(
                globset::Glob::new(p)
                    .map_err(|e| FsError::InvalidInput(e.to_string()))?
                    .compile_matcher(),
            ),
            None => None,
        };
        let mut candidates: Vec<Candidate> = Vec::with_capacity(resp.candidates.len());
        // Reconstruct full workspace-relative paths for hit parity with local
        // grep (reviewer M1): the backend returns folder_id + name, but local
        // grep yields the folder-relative path, and `path_glob` (e.g.
        // `src/**/*.rs`) must match against that full path, not the bare
        // filename. Build a folder_id -> relative-path map once from the folder
        // tree, then join the candidate name onto its parent's path.
        let folder_paths = self.folder_path_map().await?;
        for c in resp.candidates {
            let path = if c.folder_id.is_empty() {
                PathBuf::from(&c.name)
            } else {
                match folder_paths.get(&c.folder_id) {
                    Some(dir) => dir.join(&c.name),
                    // Folder not in the map (created after the walk, or past the
                    // MAX_FOLDERS bound): fall back to the bare name rather than
                    // dropping the hit.
                    None => PathBuf::from(&c.name),
                }
            };
            if let Some(g) = &path_glob
                && !g.is_match(&path)
            {
                continue;
            }
            candidates.push(Candidate {
                path,
                text: c.extracted_text,
            });
        }

        // Phase 2: in-process linear-time re-match (dialect parity with local).
        let mut hits = rematch::rematch(&re, &candidates, &args);

        // Session-dirty union: files written this session may not be indexed
        // yet (async Kafka). Re-match their in-process content so a just-written
        // file is grep-visible, matching LocalFilesystem read-after-write. Reads
        // dirty files through this same trait (their content is served from the
        // index once ready; before that, write path would supply bytes). Bounded
        // by the dirty-set size. (Empty until write/append are wired.)
        let dirty = self.dirty_snapshot();
        if !dirty.is_empty() && hits.len() < args.max_hits {
            let mut dirty_cands: Vec<Candidate> = Vec::new();
            for p in dirty {
                if let Some(g) = &path_glob
                    && !g.is_match(&p)
                {
                    continue;
                }
                if let Ok(bytes) = self.read(&p, None).await
                    && let Ok(text) = String::from_utf8(bytes.to_vec())
                {
                    dirty_cands.push(Candidate { path: p, text });
                }
            }
            let mut extra = rematch::rematch(&re, &dirty_cands, &args);
            hits.append(&mut extra);
            hits.truncate(args.max_hits);
        }

        Ok(hits)
    }
}
