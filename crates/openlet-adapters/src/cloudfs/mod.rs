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

mod literals;
mod rematch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use openlet_core::adapters::filesystem::{
    ByteRange, DirEntry, FileMeta, Filesystem, GlobOpts, GrepArgs, GrepHit, WriteOpts,
};
use openlet_core::error::FsError;
use tonic::transport::Channel;
use tonic::{Request, Status};

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

    /// List EVERY child folder of `parent` (root when `None`), following
    /// `next_page_token` to exhaustion. The single-page `page_size: 1000` calls
    /// this replaced silently dropped children past the first page — making a
    /// legitimate file past #1000 resolve to `NotFound` and `list`/glob omit
    /// entries. Bounded by `MAX_PAGES` so a backend paging bug can't loop
    /// forever.
    async fn list_all_folders(&self, parent: Option<&str>) -> Result<Vec<pb::Folder>, FsError> {
        let mut out: Vec<pb::Folder> = Vec::new();
        let mut token = String::new();
        for _ in 0..MAX_PAGES {
            let resp = self
                .client()
                .list_folders(self.authed(pb::ListFoldersRequest {
                    workspace_id: self.workspace_id.clone(),
                    parent_folder_id: parent.unwrap_or_default().to_string(),
                    page_size: PAGE_SIZE,
                    page_token: token,
                })?)
                .await
                .map_err(status_to_fs)?
                .into_inner();
            out.extend(resp.folders);
            if resp.next_page_token.is_empty() {
                return Ok(out);
            }
            token = resp.next_page_token;
        }
        Err(FsError::Io(
            "cloud list_folders exceeded max pages (backend pagination loop?)".into(),
        ))
    }

    /// List EVERY file directly in `folder` (root when `None`), following
    /// `next_page_token` to exhaustion. See [`Self::list_all_folders`] for why
    /// single-page listing was unsafe.
    async fn list_all_files(&self, folder: Option<&str>) -> Result<Vec<pb::FileInfo>, FsError> {
        let mut out: Vec<pb::FileInfo> = Vec::new();
        let mut token = String::new();
        for _ in 0..MAX_PAGES {
            let resp = self
                .client()
                .list_files(self.authed(pb::ListFilesRequest {
                    workspace_id: self.workspace_id.clone(),
                    folder_id: folder.unwrap_or_default().to_string(),
                    page_size: PAGE_SIZE,
                    page_token: token,
                })?)
                .await
                .map_err(status_to_fs)?
                .into_inner();
            out.extend(resp.files);
            if resp.next_page_token.is_empty() {
                return Ok(out);
            }
            token = resp.next_page_token;
        }
        Err(FsError::Io(
            "cloud list_files exceeded max pages (backend pagination loop?)".into(),
        ))
    }

    /// Record a path in the session-dirty set (dedup on insert). Reserved for
    /// the `write`/`append` path (stubbed this phase); see `session_dirty` on
    /// why `rename` must NOT call it.
    #[allow(dead_code, reason = "wired when write/append land; see session_dirty doc")]
    fn mark_dirty(&self, path: &Path) {
        let mut d = self.session_dirty.lock().expect("session_dirty poisoned");
        let p = path.to_path_buf();
        if !d.contains(&p) {
            d.push(p);
        }
    }

    /// Snapshot the session-dirty paths.
    fn dirty_snapshot(&self) -> Vec<PathBuf> {
        self.session_dirty
            .lock()
            .expect("session_dirty poisoned")
            .clone()
    }

    /// Resolve a workspace-relative path to a `(folder_id, file_name)` pair by
    /// walking the folder tree. `folder_id` is `None` for a workspace-root
    /// file. The final segment is treated as the file name; leading segments
    /// are folders matched by name.
    async fn resolve_parent_folder(&self, path: &Path) -> Result<(Option<String>, String), FsError> {
        let rel = normalize_rel(path)?;
        let segments: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        let Some((file_name, dirs)) = segments.split_last() else {
            return Err(FsError::InvalidInput("empty path".into()));
        };

        let mut parent: Option<String> = None;
        for dir in dirs {
            let folders = self.list_all_folders(parent.as_deref()).await?;
            // Detect duplicate names in one parent: resolution would otherwise
            // pick an arbitrary row, so read/remove/rename could hit the wrong
            // folder. Fail loud instead.
            let mut matches = folders.into_iter().filter(|f| f.name == *dir);
            match matches.next() {
                None => return Err(FsError::NotFound(rel.clone())),
                Some(_) if matches.next().is_some() => {
                    return Err(FsError::InvalidInput(format!(
                        "ambiguous path: multiple folders named {dir:?} under the same parent"
                    )));
                }
                Some(f) => parent = Some(f.folder_id),
            }
        }
        Ok((parent, (*file_name).to_string()))
    }

    /// Resolve a workspace-relative path to a `file_id`.
    async fn resolve_file_id(&self, path: &Path) -> Result<String, FsError> {
        let (folder, name) = self.resolve_parent_folder(path).await?;
        let rel = normalize_rel(path)?;
        let files = self.list_all_files(folder.as_deref()).await?;
        let mut matches = files.into_iter().filter(|f| f.name == name);
        match matches.next() {
            None => Err(FsError::NotFound(rel)),
            Some(_) if matches.next().is_some() => Err(FsError::InvalidInput(format!(
                "ambiguous path: multiple files named {name:?} under the same folder"
            ))),
            Some(f) => Ok(f.file_id),
        }
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
                let start = usize::try_from(r.start).unwrap_or(usize::MAX).min(full.len());
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

    async fn write(&self, _path: &Path, _body: Bytes, _opts: WriteOpts) -> Result<FileMeta, FsError> {
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

impl CloudFilesystem {
    /// Resolve a directory path to its folder id (`None` = workspace root).
    /// Unlike `resolve_parent_folder`, every segment is a folder.
    async fn resolve_folder_id_for_dir(&self, path: &Path) -> Result<Option<String>, FsError> {
        let rel = normalize_rel(path)?;
        let segments: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        let mut parent: Option<String> = None;
        for dir in segments {
            let folders = self.list_all_folders(parent.as_deref()).await?;
            parent = Some(unique_named(&folders, dir, |f| &f.name, |f| f.folder_id.clone(), &rel)?);
        }
        Ok(parent)
    }

    /// Recursively collect every file path in the workspace as
    /// `(relative_path, file_id)`. Bounded breadth-first walk of the folder
    /// tree. Used by `glob`.
    async fn walk_all_files(&self) -> Result<Vec<(PathBuf, String)>, FsError> {
        let mut out: Vec<(PathBuf, String)> = Vec::new();
        // (folder_id, prefix path) queue; root folder_id = None.
        let mut queue: Vec<(Option<String>, PathBuf)> = vec![(None, PathBuf::new())];
        let mut visited = 0usize;
        const MAX_FOLDERS: usize = 10_000;

        while let Some((folder_id, prefix)) = queue.pop() {
            visited += 1;
            if visited > MAX_FOLDERS {
                break;
            }
            for f in self.list_all_files(folder_id.as_deref()).await? {
                out.push((prefix.join(&f.name), f.file_id));
            }
            for sub in self.list_all_folders(folder_id.as_deref()).await? {
                queue.push((Some(sub.folder_id), prefix.join(&sub.name)));
            }
        }
        Ok(out)
    }

    /// Build a `folder_id -> workspace-relative folder path` map by walking the
    /// folder tree once. Used to reconstruct full paths for grep hits (reviewer
    /// M1): the backend returns each candidate's `folder_id` + file `name`, and
    /// this map turns that into the same workspace-relative path
    /// `LocalFilesystem::grep` produces, so hit paths AND `path_glob` matching
    /// behave identically across backends. Bounded by `MAX_PAGES`-paginated
    /// listings and the folder count.
    async fn folder_path_map(&self) -> Result<HashMap<String, PathBuf>, FsError> {
        let mut map: HashMap<String, PathBuf> = HashMap::new();
        let mut queue: Vec<(Option<String>, PathBuf)> = vec![(None, PathBuf::new())];
        let mut visited = 0usize;
        const MAX_FOLDERS: usize = 10_000;
        while let Some((folder_id, prefix)) = queue.pop() {
            visited += 1;
            if visited > MAX_FOLDERS {
                break;
            }
            for sub in self.list_all_folders(folder_id.as_deref()).await? {
                let path = prefix.join(&sub.name);
                map.insert(sub.folder_id.clone(), path.clone());
                queue.push((Some(sub.folder_id), path));
            }
        }
        Ok(map)
    }
}

/// Find the single entry in `items` whose name equals `want`, returning its
/// mapped value. Errors `NotFound` when absent and `InvalidInput` when the
/// backend returns MORE THAN ONE match (ambiguous — resolving to an arbitrary
/// row could target the wrong file; reviewer M4). `rel` is the original path
/// for error context.
fn unique_named<T, N, V>(
    items: &[T],
    want: &str,
    name_of: N,
    value_of: V,
    rel: &str,
) -> Result<String, FsError>
where
    N: Fn(&T) -> &str,
    V: Fn(&T) -> String,
{
    let mut matches = items.iter().filter(|it| name_of(it) == want);
    let Some(first) = matches.next() else {
        return Err(FsError::NotFound(rel.to_string()));
    };
    if matches.next().is_some() {
        return Err(FsError::InvalidInput(format!(
            "ambiguous path '{rel}': multiple entries named '{want}' in one folder"
        )));
    }
    Ok(value_of(first))
}

/// Normalize a workspace-relative path to a forward-slash string, rejecting
/// absolute paths and `..` escapes (workspace-boundary invariant the trait
/// imposes on every impl).
fn normalize_rel(path: &Path) -> Result<String, FsError> {
    if path.is_absolute() {
        return Err(FsError::OutsideWorkspace(path.display().to_string()));
    }
    let mut parts: Vec<String> = Vec::new();
    for comp in path.components() {
        use std::path::Component;
        match comp {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(FsError::OutsideWorkspace(path.display().to_string()));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::OutsideWorkspace(path.display().to_string()));
            }
        }
    }
    Ok(parts.join("/"))
}

/// Map a `FileInfo` to trait `FileMeta`. `mtime_ms` from `updated_at`;
/// `is_binary` inferred from the magika label (text vs not).
fn file_info_to_meta(info: &pb::FileInfo) -> FileMeta {
    let mtime_ms = info
        .updated_at
        .as_ref()
        .map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
        .unwrap_or(0);
    // Heuristic parity with local `is_binary`: treat non-text magika labels as
    // binary. file-service classifies via Magika at extraction time.
    let is_binary = !info.magika_label.is_empty()
        && !info.magika_label.starts_with("text")
        && info.magika_label != "txt"
        && info.magika_label != "markdown"
        && info.magika_label != "code";
    FileMeta {
        size: u64::try_from(info.size_bytes).unwrap_or(0),
        mtime_ms,
        is_binary,
        sha256: None,
    }
}

/// Map a gRPC `Status` to an `FsError`. `NotFound` → `FsError::NotFound`;
/// everything else (including `Unimplemented` from a not-yet-deployed backend)
/// → `FsError::Io` carrying the code + message.
fn status_to_fs(s: Status) -> FsError {
    match s.code() {
        tonic::Code::NotFound => FsError::NotFound(s.message().to_string()),
        tonic::Code::InvalidArgument => FsError::InvalidInput(s.message().to_string()),
        tonic::Code::Unimplemented => FsError::Io(format!(
            "file-service RPC unimplemented (deploy skew? backend must ship the \
             GrepFiles/proto revision before cloud mode enables): {}",
            s.message()
        )),
        code => FsError::Io(format!("file-service gRPC {code:?}: {}", s.message())),
    }
}

pub use pb::file_service_client::FileServiceClient as GeneratedFileServiceClient;
