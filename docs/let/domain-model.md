# Domain Model

## Overview

This document defines the canonical terms and relationships of the Openlet
knowledge system. It is the authority for object meaning and cardinality.

## Conceptual Model

```text
Workspace
 ├── /sources      contains Source files and user folders
 ├── /artifacts    contains one Artifact Root per processed Source
 └── /brain        contains Brain folders and Lets

Source ──processed by──► Pipeline ──writes──► Artifact Root
Artifact Root ──read by──► Fixed Source Renderer
Artifact Root ──read by──► Indexer Agent ──writes──► Let
Let ──contains──► Derived Files
Let ──references──► Source (many-to-many, with optional locators)
```

## Core Entities

### Workspace

A Workspace is the user-visible security and collaboration boundary. It owns
one filesystem tree and exactly three reserved roots: `/sources`,
`/artifacts`, and `/brain`. It is not a storage volume; `volume_id` would only
repeat information already expressed by workspace and root.

### Source

A Source is an original user-managed file beneath `/sources`. It can be a
document, spreadsheet, image, audio file, video, text note, saved article, or
another supported input. It is an input and provenance object, not a Let.

### Pipeline

A Pipeline is a bounded Phase 1 processor for exactly one Source. It classifies
and decomposes the Source, then writes files for a fixed renderer and agent
consumption. It has no Brain access and no authority to decide Let boundaries.

### Artifact Root and Artifact

An Artifact Root is the pipeline-managed directory for one Source:
`/artifacts/{source-file-id}`. It contains a manifest and arbitrary
pipeline-specific files. An Artifact is any file inside that root.

Artifacts are durable internal representations, not temporary prompt chunks.
The product term is distinct from Leti runtime `ArtifactStore`, which stores
session-scoped runtime blobs.

### Brain

The Brain is the agent-managed knowledge tree beneath `/brain`. It contains
Folders and Lets. It is not an alternative raw-file explorer or an artifact
dump.

### Folder

A Folder is a navigational container for multiple Lets or subfolders. It has
no requirement to be independently useful as knowledge. A broad area with
several independent responsibilities belongs at Folder level, not inside one
oversized Let.

### Let

A Let is the smallest user-addressable derived document that is independently
useful, has one stable semantic scope, and is maintained as one unit. It may be
one file or a directory containing many derived files.

It is not a raw upload, search chunk, embedding record, mandatory Source
wrapper, or merely a summary. It is the primary user-visible knowledge unit in
the Brain.

### Derived File

A Derived File is a file within a Let. The agent may copy an Artifact, edit it,
reorganize it, combine it with other material, or write it anew. These are
ordinary filesystem operations; no separate promotion/transform domain model
is required.

### Renderer

A Fixed Source Renderer is a registered type-specific UI for one processed
Source. A Generative Let Renderer is a controlled component composition for a
Let based on its semantic content. They are different products.

### Provenance Link

A Provenance Link records that a Let uses a Source. It MAY include page,
section, sheet-range, timestamp, or artifact-path locators. It does not mean
the Let copied all source content or that the Source belongs only to that Let.

## Relationship Cardinality

| Relationship | Cardinality | Meaning |
|---|---|---|
| Workspace → Source | One to many | A workspace owns all Sources in its source root |
| Source → Artifact Root | Zero or one active root | A Source may be unprocessed or failed; a ready Source has one active root |
| Artifact Root → Artifact | One to many | Pipelines choose internal structure |
| Workspace → Let | One to many | Lets live in the Workspace Brain |
| Source ↔ Let | Many to many | Upload and knowledge boundaries are independent |
| Let → Derived File | One to many | A Let may be a file or a structured directory |
| Folder → Let | One to many | Folders organize Lets |

All of the following are valid:

```text
One Source → no Let yet
One Source → one Let
One Source → many Lets
Many Sources → one Let
Many Sources → many overlapping Lets
```

## Ownership and Mutability

| Object | Primary writer | User visibility | Notes |
|---|---|---|---|
| Source and source folder | User | Full Files explorer | Original tree |
| Artifact Root | Pipeline | Indirect through fixed renderer | No raw artifact browsing |
| Let and Brain folder | Phase 2 agent | Brain UI | User mutation policy deferred |
| Provenance Link | Agent/system | Source and Let detail | Must remain inspectable |
| Processing job | System | Status only | Operational, not content |

## Invariants

1. Every Source, Artifact Root, and Let belongs to exactly one Workspace.
2. A Source's active Artifact Root is keyed by stable Source ID, never by a
   user-controlled filename or path.
3. Let boundaries are independent of source count, byte size, and file format.
4. An Artifact cannot become a Let merely because it is readable.
5. A provenance link is required whenever a Let materially uses a Source.
6. Brain navigation shows Folders and Lets, not raw Artifacts.
7. A Folder may be broad; a Let must have one stable semantic scope.
8. The filesystem stores content; coordination metadata stays minimal.

## Identity and Naming

Stable identifiers are required for Workspace, Source, and Let. Paths and
titles are presentation and navigation data; they may change without changing
identity. Every Let MUST have a human-readable title and stored scope statement
used by later agent runs. See [Let Boundary](./let-boundary.md).

## Source, Artifact, and Let Compared

| Question | Source | Artifact Root | Let |
|---|---|---|---|
| What is it? | Original user file | File-local processed representation | Derived user-visible knowledge document |
| Who writes it? | User | Pipeline | Agent |
| Boundary | Upload/file | One Source | Semantic responsibility and lifecycle |
| Who browses it? | User | Renderer/agent | User through Brain |
| May use many Sources? | No | No | Yes |
| May one Source yield many? | N/A | No | Yes |

## References

- [Workspace Filesystem](./workspace-filesystem.md)
- [Let Model](./let-model.md)
- [Let Boundary](./let-boundary.md)
- [Brain Integration](./brain-integration.md)
