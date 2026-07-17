# Openlet Let Design

## Overview

This directory is the normative product and architecture specification for the
Openlet knowledge system. It replaces the former single-file foundation with
separate documents that each own one concern.

Openlet preserves original files, digests them into structured artifacts, and
lets an agent turn those artifacts into durable knowledge in the Brain. A
**Let** is the user-visible unit of derived knowledge in that Brain.

```text
User-managed originals        System-managed representation       Agent-managed knowledge

/sources                      /artifacts                          /brain
  upload, organize              digest one Source                    folders and Lets
  retain file control           fixed source UI                      generative Let UI
```

## Reading Order

1. [Product Foundation](./product-foundation.md) — problem, user promise, and
   product principles.
2. [Domain Model](./domain-model.md) — canonical meanings and relationships
   of Source, Artifact, Brain, Folder, Let, and provenance.
3. [Workspace Filesystem](./workspace-filesystem.md) — the three fixed roots
   and shared filesystem design.
4. [Let Model](./let-model.md) and [Let Boundary](./let-boundary.md) — what a
   Let contains and how the agent creates, updates, splits, merges, or links
   Lets.
5. [Source Processing](./source-processing.md) and
   [Brain Integration](./brain-integration.md) — the two asynchronous phases.

## Document Map

| Document | Owns | Read when deciding |
|---|---|---|
| [product-foundation.md](./product-foundation.md) | Product thesis, principles, user surfaces, non-goals | Why the product behaves this way |
| [domain-model.md](./domain-model.md) | Terms, cardinality, invariants, identity | What an object means |
| [workspace-filesystem.md](./workspace-filesystem.md) | Paths, roots, generic filesystem reuse | Where content lives |
| [access-control-and-apis.md](./access-control-and-apis.md) | Actors, scoped mounts, API boundaries | Who may read or write what |
| [source-processing.md](./source-processing.md) | Phase 1 pipelines, manifests, artifact output | How one Source is digested |
| [source-rendering.md](./source-rendering.md) | Fixed source views | How a processed Source is displayed |
| [brain-integration.md](./brain-integration.md) | Phase 2 agent flow and provenance | How artifacts enter the Brain |
| [let-model.md](./let-model.md) | Let anatomy, files, metadata, generative UI | What a Let is made of |
| [let-boundary.md](./let-boundary.md) | Scope contract and lifecycle operations | Whether to create, update, split, merge, or link |
| [user-experience.md](./user-experience.md) | Files, Brain, Ask, status, navigation | What the user sees and controls |
| [processing-reliability.md](./processing-reliability.md) | State, retries, atomic publication, reprocessing | How processing remains safe |
| [scenarios.md](./scenarios.md) | End-to-end examples | How the model behaves in practice |
| [prototype-integration.md](./prototype-integration.md) | Current Openlet and Leti ownership seams | How the prototypes map to the target model |
| [requirements.md](./requirements.md) | Functional and non-functional requirements | What implementation must satisfy |
| [decisions-and-open-questions.md](./decisions-and-open-questions.md) | Accepted decisions and intentional deferrals | What is locked versus undecided |

## Normative Language

The words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are
normative. A document may explain a decision, but the document named in the
table above is the sole authority for that decision's detailed rules.

When documents appear to conflict, use this precedence:

1. `decisions-and-open-questions.md` for an explicitly accepted decision.
2. The document that owns the relevant concern.
3. This index only as navigation and summary.
4. Historical material in `drafts/` only as context; it is not normative.

## Core Model in One Page

- Openlet has **one workspace type**, not a separate normal and AI workspace.
- Each workspace has three reserved roots: `/sources`, `/artifacts`, and
  `/brain`.
- Users manage files and folders under `/sources` directly.
- Pipelines process exactly one Source at a time and write reusable artifacts
  under `/artifacts/{source-file-id}`.
- Users see a processed Source through a fixed, type-specific renderer; they
  do not browse the raw artifact tree.
- A background agent reads artifacts and writes Lets into `/brain`.
- A Source can contribute to zero, one, or many Lets. A Let can use one or many
  Sources.
- A Let is not a summary or an upload wrapper. It is the smallest
  independently useful, user-addressable derived document with one stable
  semantic scope and one maintenance lifecycle.
- Folder boundaries organize multiple Lets. Let boundaries organize one
  independently useful knowledge responsibility.
- Access control is enforced by reserved root and scoped API/mount, not by
  arbitrary folder-level ACLs.

## Change Protocol

Any proposed implementation that changes a foundational invariant MUST first
update the owning document and, when it changes a locked choice, add a decision
entry. In particular, do not introduce a second filesystem, `volume_id`, raw
artifact browsing, a one-Source-to-one-Let assumption, or a separate
promotion/transform domain model without an explicit replacement decision.

## References

- Historical discussion: `drafts/draft-1.md` through `drafts/draft-5.md`
- Current agent runtime: `docs/architecture.md`
- Current runtime integration seam: `docs/integration-guide.md`
- Sibling file-management backend: `../openlet/docs/system-architecture.md`
