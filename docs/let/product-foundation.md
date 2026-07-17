# Product Foundation

## Overview

Openlet is a long-term personal data and knowledge system for people who have
many scattered files, saved articles, images, notes, and ideas but cannot know
in advance which material will matter later.

The product is neither a cloud drive with a chatbot attached nor a transient
RAG index over uploads. It preserves the original file, makes that file useful
quickly, and continuously builds durable knowledge from it.

## Product Thesis

```text
Capture
  → preserve the source
  → digest the source
  → present the source intelligently
  → absorb useful knowledge into the Brain
  → retrieve, discuss, and synthesize later
```

The source remains useful as a file. The Brain becomes useful as organized
knowledge. Neither replaces the other.

## User Promise

Openlet MUST let a user:

- Drop files, links, notes, images, and media into a durable home with little
  up-front organization.
- Keep original files and manually organize them when that control matters.
- Open a processed file through a UI appropriate to its type without waiting
  for broader agent reasoning.
- Let background work convert material into durable knowledge without forcing
  the user to decide every folder, tag, or summary first.
- Find, inspect, discuss, compare, and reuse knowledge later with visible
  provenance back to original Sources.

## Product Principles

### Preserve before interpreting

The original is a user asset. Interpretation MUST NOT require discarding the
original or replacing it with an agent-generated derivative.

### Source control and knowledge control are different jobs

Users need ordinary file management for originals. Agents need a stable derived
space in which to construct knowledge. One workspace with reserved roots gives
both groups the correct control without making either workflow second-class.

### Immediate value and long-term value are separate phases

Phase 1 gives a file-local result: extraction, structure, preview, and a fixed
renderer. Phase 2 gives a cross-source result: Lets, relationships, retrieval,
and generative UI. Phase 2 may be slow or retried without making Phase 1
unavailable.

### Knowledge is not constrained by upload boundaries

An upload is a transport and provenance boundary. A Let is a user and semantic
boundary. One does not determine the other.

### Provenance is part of trust

The user must be able to move from a Let to its supporting Sources and from a
Source to the Lets that use it. This enables inspection, deletion policy, and
later reprocessing.

### Intelligence should be visible but not magical

The agent may create, update, split, merge, and connect Lets, but it acts
against explicit stored scope and provenance. It must not hide Sources,
silently overwrite unrelated knowledge, or create opaque structures.

### Manual work remains legitimate

The product begins with automatic processing, but it leaves room for manual
source organization and future human-in-the-loop controls. Automation is not a
reason to remove user agency.

## One Workspace Type

Openlet uses one logical workspace type. A separate AI-only workspace would
force users to understand two products, complicate ownership and sharing, and
turn ordinary data into a second-class input.

One workspace contains both user-managed Sources and agent-managed Brain
content, separated by reserved roots and permissions.

## The Two Product Phases

| Phase | Scope | Owner | Immediate output | User value |
|---|---|---|---|---|
| Phase 1: File Digestion | One Source | Pipeline | Artifact tree and fixed renderer | Read, inspect, search, and preview one file intelligently |
| Phase 2: Brain Integration | Artifacts plus Brain | Background agent | Lets, derived files, provenance, generative Let UI | Reuse, connect, ask, and synthesize knowledge |

Phase 1 MUST NOT inspect the Brain, decide Let boundaries, merge Sources, or
reorganize user files. Phase 2 is contextual and MAY compare incoming material
with existing Lets.

## User Surfaces

```text
Files | Brain | Ask
```

- **Files** is the normal source explorer: upload, folder navigation, move,
  rename, preview, download, and processing status.
- **Brain** is the agent-managed knowledge tree: folders and Lets, not raw
  pipeline artifacts.
- **Ask** is conversational retrieval and discussion over the Brain with
  provenance to Lets and Sources.

## Non-Goals

This foundation does not require a complete Google Drive replacement, multiple
user-visible workspace types, folder-level ACLs, raw artifact browsing, a Let
per upload, a summary-only Let, generative UI for Sources, persistent content
version history, or final HITL/deletion/collaboration/retrieval policy.

## Success Condition

The model works when a user can upload material and leave, return to a useful
source view, and eventually find a coherent Let that can be understood,
discussed, and traced back to original material without having predicted the
final knowledge structure before upload.

## References

- [Domain Model](./domain-model.md)
- [Source Processing](./source-processing.md)
- [Brain Integration](./brain-integration.md)
- [User Experience](./user-experience.md)
