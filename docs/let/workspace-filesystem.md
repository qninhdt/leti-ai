# Workspace Filesystem

## Overview

Each Workspace has one logical filesystem tree with three reserved top-level
roots. The roots create clear ownership and permission boundaries while
reusing one generic file/folder implementation, one metadata model, and the
same local or object-storage adapters.

```text
/workspace-{workspace-id}/
├── sources/
├── artifacts/
└── brain/
```

The roots are created with the Workspace. Ordinary users and agents MUST NOT
rename, move, or delete them.

## Root Responsibilities

| Root | Owner | Contents | Direct user experience |
|---|---|---|---|
| `/sources` | User | Original files and ordinary folders | File explorer with normal management actions |
| `/artifacts` | Phase 1 pipeline | Structured file-local output and manifests | Hidden; exposed only through fixed source views |
| `/brain` | Phase 2 agent | Folders, Lets, and derived files | Brain explorer and Let views |

The roots are namespaces in one Workspace filesystem. They are not separate
products, storage volumes, or file/folder tables.

## Source Root

`/sources` is a normal user-managed hierarchy. The user MAY upload, create
folders, move, rename, download, delete, and search according to workspace
role. The system does not require perfect manual organization before upload.

```text
/sources/
├── Books/
│   └── example-book.pdf
├── Finance/
│   └── personal-budget-2026.xlsx
├── Research/
│   └── agent-memory-paper.pdf
└── Notes/
    └── openlet-idea.md
```

Moving or renaming a Source changes only its user-facing path. It MUST NOT
break artifact or provenance identity because those relations use stable Source
ID.

## Artifact Root

Each processed Source has one active Artifact Root:

```text
/artifacts/
└── {source-file-id}/
    ├── manifest.json
    └── pipeline-specific-output...
```

The first segment is a stable Source ID, never a filename. Beneath that
directory, each pipeline chooses folders and files best suited to its renderer
and agent reading. The shared filesystem does not impose a flat artifact list
or universal artifact schema beyond the required manifest.

Artifacts are retained internal system files. They can be nested and rich
because only pipeline services, renderers, and agents access them directly.

## Brain Root

`/brain` is the agent-managed knowledge tree:

```text
/brain/
├── Learn/
│   └── Distributed Systems/
│       └── Designing Data-Intensive Applications/
├── Finance/
│   └── Personal Finance 2026/
└── Ideas/
    └── Openlet Product Direction/
```

Folders are navigational groupings. A Let is a user-visible derived document
whose internal representation may be a file or directory. The top-level
taxonomy is deliberately not fixed by this foundation.

## One Generic Filesystem

The generic filesystem remains responsible for:

- Creating, listing, moving, renaming, and deleting nodes.
- Reading and writing files.
- Resolving paths and persisting bytes.
- Mapping bytes to local disk or object storage.

Do not create separate Source, Artifact, and Brain tables merely because the
roots have different product meaning. Specialized coordination data belongs in
minimal metadata and provenance relations, not duplicated CRUD systems.

`volume_id` is explicitly unnecessary. A node has a Workspace identity and a
path beneath one reserved root; a volume dimension would repeat that fact.

## Scoped Filesystem Wrappers

Each actor receives a root-scoped wrapper over the shared filesystem:

```text
SourceFilesystem(root="/sources")
ArtifactFilesystem(root="/artifacts")
BrainFilesystem(root="/brain")
```

Each wrapper MUST:

1. Accept only relative paths from its caller.
2. Normalize the path before resolution.
3. Reject absolute paths, root replacement, and traversal outside its scope.
4. Resolve symlinks or equivalent indirections safely so they cannot escape
   the root.
5. Prefix the reserved root internally.
6. Enforce the caller's operation policy before filesystem access.
7. Never expose a caller-controlled root selector.

For example, a Files client sends `Finance/personal-budget-2026.xlsx`, not
`/sources/Finance/personal-budget-2026.xlsx` and never `/artifacts/...`.

## Path and Lifecycle Rules

- Reserved roots cannot be deleted or renamed through ordinary operations.
- A pipeline writes only under the Artifact Root assigned to its input Source.
- An agent writes only under `/brain` through its Brain mount.
- The agent normally receives artifacts at `/inputs/...`, not a raw workspace
  root, so it cannot accidentally read user-managed originals.
- A Let's path is not its semantic identity. It may move within the Brain
  without changing `let_id` or provenance links.
- Source deletion, artifact cleanup, and cascade behavior remain deferred. No
  implementation may silently delete a Let because a Source moved.

## Acceptance Criteria

- [ ] Workspace creation initializes all three reserved roots.
- [ ] A user can manage files and folders under `/sources`.
- [ ] A user-facing Files API cannot resolve an Artifact or Brain path.
- [ ] Renaming a Source does not change its Artifact Root key.
- [ ] Source, Artifact, and Brain operations reuse one filesystem implementation.
- [ ] No schema or service boundary requires `volume_id`.

## References

- [Domain Model](./domain-model.md)
- [Access Control and APIs](./access-control-and-apis.md)
- [Source Processing](./source-processing.md)
- [Let Model](./let-model.md)
