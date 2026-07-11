//! Path/folder resolution + folder-tree walks for the cloud filesystem.
//!
//! Split out of `mod.rs`: these inherent methods turn workspace-relative
//! paths into file-service `folder_id`/`file_id` handles and enumerate the
//! folder tree for `glob`/`grep`. All are `CloudFilesystem` methods; a
//! submodule can reach the parent struct's private fields + helpers, so the
//! split is purely organizational.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use openlet_core::error::FsError;

use super::convert::{normalize_rel, unique_named};
use super::{CloudFilesystem, MAX_PAGES, PAGE_SIZE, pb};

/// Hard ceiling on folders visited in a full tree walk, so a workspace with a
/// pathological folder count (or a backend paging bug) cannot make `glob`/
/// `grep` walk forever. Shared by both tree walks below.
const MAX_FOLDERS: usize = 10_000;

impl CloudFilesystem {
    /// List EVERY child folder of `parent` (root when `None`), following
    /// `next_page_token` to exhaustion. The single-page `page_size: 1000` calls
    /// this replaced silently dropped children past the first page — making a
    /// legitimate file past #1000 resolve to `NotFound` and `list`/glob omit
    /// entries. Bounded by `MAX_PAGES` so a backend paging bug can't loop
    /// forever.
    pub(super) async fn list_all_folders(
        &self,
        parent: Option<&str>,
    ) -> Result<Vec<pb::Folder>, FsError> {
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
                .map_err(super::convert::status_to_fs)?
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
    pub(super) async fn list_all_files(
        &self,
        folder: Option<&str>,
    ) -> Result<Vec<pb::FileInfo>, FsError> {
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
                .map_err(super::convert::status_to_fs)?
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

    /// Resolve a workspace-relative path to a `(folder_id, file_name)` pair by
    /// walking the folder tree. `folder_id` is `None` for a workspace-root
    /// file. The final segment is treated as the file name; leading segments
    /// are folders matched by name.
    pub(super) async fn resolve_parent_folder(
        &self,
        path: &Path,
    ) -> Result<(Option<String>, String), FsError> {
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
    pub(super) async fn resolve_file_id(&self, path: &Path) -> Result<String, FsError> {
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

    /// Resolve a directory path to its folder id (`None` = workspace root).
    /// Unlike `resolve_parent_folder`, every segment is a folder.
    pub(super) async fn resolve_folder_id_for_dir(
        &self,
        path: &Path,
    ) -> Result<Option<String>, FsError> {
        let rel = normalize_rel(path)?;
        let segments: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        let mut parent: Option<String> = None;
        for dir in segments {
            let folders = self.list_all_folders(parent.as_deref()).await?;
            parent = Some(unique_named(
                &folders,
                dir,
                |f| &f.name,
                |f| f.folder_id.clone(),
                &rel,
            )?);
        }
        Ok(parent)
    }

    /// Recursively collect every file path in the workspace as
    /// `(relative_path, file_id)`. Bounded breadth-first walk of the folder
    /// tree. Used by `glob`.
    ///
    /// NB: kept as its own BFS (not merged with [`Self::folder_path_map`])
    /// deliberately — the two walks bound `MAX_FOLDERS` against a different
    /// set (files-of-popped-folders here vs discovered-child-folders there),
    /// so merging them would shift which entries survive the DoS cutoff on a
    /// >10k-folder workspace. Keeping them separate preserves that behavior.
    pub(super) async fn walk_all_files(&self) -> Result<Vec<(PathBuf, String)>, FsError> {
        let mut out: Vec<(PathBuf, String)> = Vec::new();
        // (folder_id, prefix path) queue; root folder_id = None.
        let mut queue: Vec<(Option<String>, PathBuf)> = vec![(None, PathBuf::new())];
        let mut visited = 0usize;

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
    pub(super) async fn folder_path_map(&self) -> Result<HashMap<String, PathBuf>, FsError> {
        let mut map: HashMap<String, PathBuf> = HashMap::new();
        let mut queue: Vec<(Option<String>, PathBuf)> = vec![(None, PathBuf::new())];
        let mut visited = 0usize;
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
