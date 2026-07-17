# Decisions and Open Questions

## Overview

This document separates accepted foundation decisions from intentional deferrals.
An unresolved item must not be implemented accidentally as a hidden assumption.

## Accepted Decisions

| ID | Decision | Rationale |
|---|---|---|
| D1 | Openlet has one logical Workspace type. | Users should not learn separate normal and AI products. |
| D2 | Every Workspace has `/sources`, `/artifacts`, and `/brain`. | One filesystem with explicit responsibilities. |
| D3 | Sources use a normal user-managed file/folder explorer. | Originals remain a durable user asset. |
| D4 | Phase 1 is file-scoped and pipeline-driven. | Extraction is bounded, predictable, retryable, and cheap. |
| D5 | Pipeline outputs are stored as agent-readable files. | Portable, inspectable representations serve renderers and agents. |
| D6 | Phase 1 uses fixed renderers selected by Source type. | Source interfaces should be stable and testable. |
| D7 | Phase 2 is asynchronous and agent-driven. | Cross-source synthesis needs Brain context. |
| D8 | A Let is not a summary. | It may retain full chapters, data, images, analysis, and indexes. |
| D9 | Source–Let relation is many-to-many. | Upload and knowledge boundaries are independent. |
| D10 | Users do not browse raw Artifacts. | Artifacts are an internal renderer/agent representation. |
| D11 | Permission uses root/API scope, not folder ACL. | Avoid unneeded permission complexity. |
| D12 | All roots reuse one generic filesystem implementation. | Avoid duplicate tables, CRUD, and storage adapters. |
| D13 | No `volume_id` is required. | Workspace plus reserved root already identifies namespace. |
| D14 | No persistent Source/Artifact version model is required now. | Undo and history are deferred. |
| D15 | HITL and confirmation policy are deferred. | They must not block the core Source → Artifact → Brain model. |
| D16 | A Let has one scope, one entry point, and one lifecycle. | Provides an operational general boundary for agent decisions. |
| D17 | Every Let stores a stable scope statement. | Prevents boundary drift across agent runs. |
| D18 | Link is distinct from merge. | Related independent Lets must not become one broad Let. |

## Explicit Non-Goals

- Multiple user-visible Workspace types.
- Folder-level ACLs.
- A complete Google Drive replacement.
- Collaborative document editing.
- Raw pipeline Artifact browsing.
- Agent access to arbitrary raw Source folders.
- One Let per upload or one Source per Let.
- Summary-only Lets.
- Autonomous Phase 1 organization of the Brain.
- Generative UI for individual Sources.
- Persistent Source or Artifact version history.
- Final HITL, approval, deletion, or cascade policy.

## Deferred Decisions

1. User mutation permission inside the Brain and Let-specific manual edits.
2. HITL thresholds, confirmation UX, and permissions for agent mutations.
3. Undo, history, revision, restore, and audit retention.
4. Source deletion behavior when Lets depend on it.
5. Artifact retention and cleanup after Source deletion.
6. Cross-workspace Source and Let relationships.
7. Whether Ask searches only Brain or may inspect selected Phase 1 Artifacts.
8. Full-text, embedding, hybrid, graph, and ranking retrieval strategy.
9. Top-level Brain taxonomy and folder-generation policy.
10. Exact Let metadata/manifest serialization.
11. Deep-analysis scheduling, cost budgets, and latency targets.
12. Final generative UI schema and component registry.
13. Long-term runtime ownership between current Python and Rust prototypes.
14. Sharing and collaboration beyond Workspace-level roles.
15. Exact graph/link relation vocabulary and user controls.

## Decision Gate for Future Changes

Create a new decision before changing a deferred item into behavior that users
can depend on, or before weakening any accepted invariant. In particular, do
not add a second workspace type, a second filesystem, raw artifact browsing, or
per-folder permissions as an implementation convenience.

## References

- [README](./README.md)
- [Let Boundary](./let-boundary.md)
- [Requirements](./requirements.md)
