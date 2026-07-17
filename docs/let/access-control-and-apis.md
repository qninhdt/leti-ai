# Access Control and API Boundaries

## Overview

Authorization is defined by actor, Workspace role, reserved root, and action.
It is not stored as an ACL on every Folder. This preserves a simple permission
model while keeping user originals, internal artifacts, and agent knowledge
isolated.

## Root Access Matrix

| Actor | `/sources` | `/artifacts` | `/brain` |
|---|---|---|---|
| Workspace owner/member | Read/write by workspace role | No direct access | Read |
| Workspace viewer | Read | No direct access | Read |
| Phase 1 pipeline worker | Read one assigned Source | Read/write assigned Artifact Root | No access |
| Phase 2 Indexer agent | No mount by default | Read assigned Artifact Roots | Read/write |
| Source renderer service | Read Source metadata | Read assigned Artifact Root | No access |
| Let renderer service | No source-tree access by default | No direct access by default | Read |

The exact user mutation policy in `/brain` is deferred. The foundation requires
at least Brain read access and does not imply that users may directly edit
agent-managed Lets.

## Why Root Scope Instead of Folder ACL

The three roots have stable product roles. Adding permission records to every
folder would create complex inheritance, moving, sharing, revocation, and
agent-tool behavior without supporting a confirmed user need.

Workspace-level roles decide whether a person can use the workspace. Root and
API scope decide which part of its filesystem a principal can access. This is
enough for the current model and does not prevent a later explicit decision on
fine-grained sharing.

## Capability-Scoped Mounts

### Phase 1 Pipeline Mount

```text
/input/original  → one assigned Source file, read-only
/output          → /artifacts/{source-file-id}, read/write
```

The worker MUST NOT inspect the Brain or unrelated Sources. It receives the
minimum authority required to digest one input file.

### Phase 2 Agent Mount

```text
/inputs          → selected /artifacts/{source-file-id} roots, read-only
/brain           → /brain, read/write
```

The agent uses ordinary file operations, for example:

```text
read  /inputs/book/manifest.json
read  /inputs/book/chapters/01.md
write /brain/Enjoy/Books/example-book/chapters/01.md
```

The agent does not need the raw `/sources` tree. This separation ensures it
works from structured, bounded artifacts rather than arbitrary binaries.

## API Surfaces

The external URL shape may evolve; these conceptual surfaces are required.

### Files API

User-facing CRUD over `/sources`:

```text
/workspaces/{workspace-id}/files/*
```

The server strips `/sources` from responses and restores it internally on
resolution. It never lets a client select another reserved root.

### Source View API

User-facing read-only representation of one processed Source:

```text
/workspaces/{workspace-id}/sources/{source-file-id}/view
```

The service verifies Source read permission, locates the active Artifact Root,
reads the manifest, selects a registered fixed renderer, and returns a safe
view model or controlled asset URLs. It MUST NOT expose a general artifact-list
or artifact-write endpoint.

### Artifact API

Internal-only pipeline and agent access:

```text
/internal/workspaces/{workspace-id}/artifacts/*
```

This is a service boundary, not a user-facing product surface.

### Brain and Let API

User read access and agent write access over `/brain`:

```text
/workspaces/{workspace-id}/brain/*
/workspaces/{workspace-id}/lets/{let-id}
```

The Let endpoint renders semantic content and registered components rather than
only returning a raw directory listing.

## Security Invariants

1. Every request is authenticated and workspace-scoped before path resolution.
2. Scoped APIs accept relative paths only and reject traversal attempts.
3. No user-facing endpoint lists raw Artifact paths or bytes without a
   renderer-specific reason.
4. A pipeline can access only its assigned input and output.
5. An agent can access only explicitly mounted Artifact Roots and `/brain`.
6. The browser never receives a broad credential that can traverse all roots.
7. Generative UI uses a registered component registry; it MUST NOT execute
   arbitrary model-generated frontend code.

## Deferred Policy

The following require later decisions: user edits inside Lets, user-created
Brain folders, share semantics beyond workspace role, agent approval policy,
and deletion/cascade behavior.

## References

- [Workspace Filesystem](./workspace-filesystem.md)
- [Source Processing](./source-processing.md)
- [Brain Integration](./brain-integration.md)
- [User Experience](./user-experience.md)
