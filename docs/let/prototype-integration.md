# Prototype Integration

## Overview

The current prototypes provide useful building blocks, but their present
boundaries are not binding on the product foundation. This document maps likely
responsibilities without requiring an immediate rewrite or selecting a final
long-term runtime owner.

## Openlet Backend

The sibling `../openlet` project already owns a cloud-native file-management
backend with workspace, file, object-storage, worker, queue, and notification
building blocks. It is the natural candidate for:

- Workspace lifecycle, membership, and workspace-level roles.
- Shared file/folder implementation.
- Source upload, source metadata, and user file APIs.
- Object storage and quota enforcement.
- Phase 1 processing workers and processing events.
- Source View API and renderer delivery.
- Background queues, status updates, and notifications.

The target model requires that these capabilities expose root-scoped views of
one filesystem rather than become three independent filesystems.

## Leti Runtime

The current `leti-ai` project is a reusable Rust agent runtime with an agent
loop, scoped filesystem adapter, tools, permissions, background lifecycle, and
plugin architecture. It is a natural candidate for Phase 2 capabilities:

- Indexer agent loop and task lifecycle.
- Read-only Artifact mounts.
- Read/write Brain mount.
- Tool execution and read-before-write behavior.
- Permission enforcement at the agent-tool boundary.
- Let creation, update, split, merge, linking, and provenance recording.

The current runtime `ArtifactStore` is session-scoped and must not be conflated
with Phase 1 pipeline Artifacts. The target integration should use a dedicated
filesystem adapter or scoped mount for workspace Artifact Roots.

## Frontend

The frontend owns:

- Files explorer and Source management actions.
- Fixed Source renderer components.
- Brain tree and Let navigation.
- Controlled generative Let renderer.
- Ask experience.
- Phase 1 and Phase 2 status presentation.

It does not execute arbitrary agent-generated frontend code or expose raw
Artifact browsing.

## Integration Boundary

```text
Openlet file-service / worker
  owns Source lifecycle and Phase 1 artifact publication
        │ source.processed event + scoped storage contract
        ▼
Leti runtime / Indexer agent
  reads Artifact Root and writes Brain Lets
        │ provenance + integration status
        ▼
Openlet frontend
  presents Files, Brain, Ask, and status
```

Authentication and transport details are host integration concerns. The product
contract is that the agent receives least-privilege artifact and Brain mounts,
not arbitrary file-service authority.

## Explicitly Deferred

The final ownership split between the Python Leti service in `../openlet` and
the Rust `leti-ai` runtime remains a separate architecture decision. The
foundation is intentionally specified in terms of capabilities so either
prototype can evolve or be replaced without changing Source, Artifact, Brain,
or Let semantics.

## References

- `../openlet/README.md`
- `../openlet/docs/system-architecture.md`
- `docs/architecture.md`
- `docs/integration-guide.md`
- [Access Control and APIs](./access-control-and-apis.md)
