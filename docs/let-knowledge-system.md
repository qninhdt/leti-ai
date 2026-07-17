# Openlet Knowledge System

## Document Control

| Field | Value |
|---|---|
| Status | Foundation draft |
| Decision date | 2026-07-17 |
| Scope | Product model, workspace model, file digestion, Brain integration, and Let semantics |
| Audience | Product, design, frontend, backend, data pipeline, and agent-runtime teams |
| Supersedes | The conflicting Source, Artifact, Workspace, and Let assumptions in `drafts/` |

## Overview

Openlet is a long-term personal data and knowledge system for people who have
many scattered files, saved articles, images, notes, and ideas but do not know
when they will need them again.

The product must let a user:

- Drop data into the system with minimal effort.
- Keep and manually organize original files when desired.
- Receive an immediately useful, type-specific experience for each processed
  file.
- Let an agent continuously absorb processed material into a durable personal
  Brain.
- Search, retrieve, ask questions, compare information, and discover insights
  across the accumulated Brain.

The architecture separates these jobs into two phases:

1. **Phase 1 — File Digestion:** a file-scoped pipeline converts one source
   file into smaller, agent-readable artifacts and powers a fixed,
   type-specific UI.
2. **Phase 2 — Brain Integration:** a background agent reads those artifacts
   and creates, updates, splits, or merges Lets in the Brain.

Both phases provide independent product value. Phase 1 makes Openlet a smart
file system. Phase 2 turns that file system into a second brain.

## Product Thesis

Openlet is not merely a cloud drive with a chatbot, and it is not merely a RAG
index over uploaded documents.

Its core value chain is:

```text
Capture
  → preserve the source
  → digest the source
  → present the source intelligently
  → absorb it into the Brain
  → retrieve, discuss, and synthesize later
```

The source remains useful as a file. The Brain becomes useful as organized
knowledge. Neither replaces the other.

## Confirmed Decisions

| ID | Decision | Rationale |
|---|---|---|
| D1 | Openlet has one logical workspace type. | Users should not need to understand separate “normal” and “AI-driven” workspace products. |
| D2 | Every workspace has three reserved filesystem roots: `/sources`, `/artifacts`, and `/brain`. | Reuses one filesystem while keeping responsibilities and permissions clear. |
| D3 | Sources use a normal file/folder explorer. | Openlet is expected to be a long-term home for originals, so users must retain direct file control. |
| D4 | Phase 1 is file-scoped and pipeline-driven. | Parsing and decomposition should be bounded, predictable, retryable, and cheaper than an autonomous agent. |
| D5 | Pipeline outputs are stored as files that an agent can read. | Markdown, text, CSV, JSON, YAML, PNG, and JPEG are portable and human-debuggable. |
| D6 | Phase 1 uses fixed renderers selected by source type. | A book, spreadsheet, image, audio file, and video need stable, testable interfaces. |
| D7 | Phase 2 is asynchronous and agent-driven. | Let boundaries and cross-source synthesis require awareness of the existing Brain. |
| D8 | A Let is a coherent knowledge package, not a summary. | A Let may contain full chapters, normalized datasets, images, analyses, indexes, and any other useful derived files. |
| D9 | A source may contribute to zero, one, or many Lets; a Let may use one or many sources. | Upload boundaries and knowledge boundaries are independent. |
| D10 | Users do not browse raw pipeline artifacts. | Artifacts are an internal representation consumed by fixed renderers and agents. |
| D11 | Permission is enforced by reserved root and API scope, not by arbitrary folder ACL. | Avoids folder-level authorization complexity while preserving isolation. |
| D12 | All three roots reuse the same file/folder implementation. | Avoids duplicate tables, CRUD logic, and storage adapters. |
| D13 | No `volume_id` abstraction is required. | The three fixed roots already provide the required namespace boundary. |
| D14 | Persistent source/artifact version models are not required in the current foundation. | Undo, audit history, and restore semantics are deferred. |
| D15 | HITL and confirmation policy are deferred. | They do not need to block the core Source → Artifact → Brain model. |

## Terminology

### Workspace

A user-visible security and collaboration boundary. A workspace owns one
filesystem tree with three reserved roots.

### Source

An original user-managed file stored under `/sources`. Examples include PDF,
DOCX, XLSX, CSV, image, audio, video, text, and Markdown files.

A Source is not a Let. Uploading one Source does not require creating one new
Let.

### Pipeline

A bounded Phase 1 processor scoped to one Source. It detects the semantic file
kind, extracts or normalizes content, and writes artifacts suitable for a
fixed renderer and for agent consumption.

A pipeline may use deterministic code, heuristics, specialized ML models, and
bounded LLM calls. It does not inspect or organize the Brain.

### Pipeline Artifact

A file produced by a Phase 1 pipeline under `/artifacts`. Examples include:

- Markdown chapters.
- Plain extracted text.
- CSV tables.
- JSON manifests and table-of-contents data.
- YAML metadata.
- OCR results.
- Thumbnails and extracted images.
- Transcripts and subtitle files.
- Video highlight metadata or clips.

Pipeline Artifact in this document is a product-level term. It is distinct
from the existing Leti runtime `ArtifactStore`, which stores session-scoped
runtime blobs.

### Brain

The agent-managed knowledge tree under `/brain`. It contains folders and Lets
that represent knowledge after Phase 2 integration.

### Let

A durable, coherent knowledge package managed by the agent inside the Brain.
Its boundary is semantic rather than based on source count, byte size, or file
format.

A Let may be represented by one file or by an internal folder tree containing
many derived files.

### Derived File

A file written into a Let by the Phase 2 agent. It may be copied from a
pipeline artifact, lightly edited, reorganized, combined with other material,
or written anew after the agent reads the relevant artifacts.

### Fixed Source UI

A predefined component set for presenting one processed Source. Examples:
`BookView`, `SpreadsheetView`, `VideoView`, and `AudioView`.

### Generative Let UI

A UI assembled for a Let from its semantic content and derived files. Unlike
the Source UI, it is not determined only by the uploaded file format.

## Workspace Filesystem

Every workspace has exactly three reserved top-level roots:

```text
/workspace-{workspace-id}/
├── sources/
├── artifacts/
└── brain/
```

These roots are created with the workspace. They cannot be renamed, moved, or
deleted by ordinary users or agents.

### Source Root

The user-managed file tree:

```text
/sources/
├── Books/
│   └── harry-potter-and-the-philosophers-stone.pdf
├── Finance/
│   └── personal-budget-2026.xlsx
├── Research/
│   └── agent-memory-paper.pdf
└── Notes/
    └── openlet-idea.md
```

### Artifact Root

The pipeline-managed internal tree:

```text
/artifacts/
├── {source-file-id}/
│   ├── manifest.json
│   └── pipeline-specific-output...
└── {another-source-file-id}/
    ├── manifest.json
    └── pipeline-specific-output...
```

The folder directly beneath `/artifacts` is keyed by stable Source file ID,
not by the user-controlled filename or Source path. Moving or renaming a
Source therefore does not break its artifact association.

Each pipeline may choose the internal folder/file structure most useful for
its renderer and the agent.

### Brain Root

The agent-managed knowledge tree:

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

Folders organize Lets. The internal structure of each Let varies with its
content.

## Why One Filesystem Is Sufficient

The three roots do not require three file tables, three folder tables, or
three storage implementations.

The existing generic filesystem remains responsible for:

- Creating and listing folders.
- Reading and writing files.
- Moving and renaming nodes.
- Deleting nodes.
- Resolving paths.
- Persisting bytes through local or cloud storage adapters.

The product exposes scoped wrappers over that filesystem:

```text
SourceFilesystem(root="/sources")
ArtifactFilesystem(root="/artifacts")
BrainFilesystem(root="/brain")
```

Each wrapper:

1. Accepts only relative paths.
2. Normalizes the requested path.
3. Rejects absolute paths and traversal outside its root.
4. Prepends its reserved root internally.
5. Applies actor-specific read/write policy.

The client never selects an arbitrary root.

## Permission Model

Permission is defined by actor, reserved root, and action. It is not stored as
an ACL on every folder.

| Actor | `/sources` | `/artifacts` | `/brain` |
|---|---|---|---|
| Workspace owner/member | Read/write according to workspace role | No direct access | Read |
| Workspace viewer | Read | No direct access | Read |
| Phase 1 pipeline worker | Read its input Source | Read/write its artifact output | No access |
| Phase 2 Indexer agent | No mount by default | Read relevant artifact roots | Read/write |
| Source renderer service | Read Source metadata | Read relevant artifact root | No access |
| Let renderer/service | No source-tree access by default | No direct access by default | Read |

The exact user mutation policy inside the Brain is deferred. The foundation
requires at least read access.

### Agent Mounts

A Phase 2 job should see a capability-scoped virtual filesystem:

```text
/inputs  → relevant `/artifacts/{source-file-id}` roots, read-only
/brain   → `/brain`, read/write
```

The agent uses ordinary filesystem operations:

```text
read /inputs/book/manifest.json
read /inputs/book/chapters/01.md
write /brain/Enjoy/Books/harry-potter-1/chapters/01.md
```

It does not need to read the raw Source tree.

### Pipeline Mounts

A Phase 1 job should receive:

```text
/input/original  → one Source file, read-only
/output          → `/artifacts/{source-file-id}`, read/write
```

The worker cannot inspect the Brain.

## API Surfaces

The exact URL shape may change, but the system needs four conceptual
interfaces.

### Files API

User-facing CRUD over `/sources`:

```text
/workspaces/{workspace-id}/files/*
```

The backend strips `/sources` from responses and automatically restores it
when resolving requests.

### Source View API

User-facing, read-only type-specific representation:

```text
/workspaces/{workspace-id}/sources/{source-file-id}/view
```

The service:

1. Verifies that the user can read the Source.
2. Locates its artifact root.
3. Reads `manifest.json`.
4. Selects the registered fixed renderer.
5. Returns a safe view model or controlled asset URLs.

The user does not receive a general artifact-list or artifact-write API.

### Artifact API

Internal pipeline/agent filesystem access:

```text
/internal/workspaces/{workspace-id}/artifacts/*
```

It is not a user-facing product surface.

### Brain and Let API

User read access and agent write access over `/brain`:

```text
/workspaces/{workspace-id}/brain/*
/workspaces/{workspace-id}/lets/{let-id}
```

The Let endpoint renders semantic content and generative UI rather than
exposing only a raw directory listing.

## Phase 1 — File Digestion

### Purpose

Phase 1 makes one file understandable and usable without requiring awareness
of any other file or Let.

It performs the expensive and specialized work that an agent should not repeat:

- File validation and type detection.
- Text and structure extraction.
- OCR.
- Table extraction.
- Media transcription.
- Chapter or section decomposition.
- Thumbnail and preview generation.
- Source-local summary and metadata extraction.
- Data normalization.
- Creation of agent-readable files.

### Scope Boundary

A Phase 1 pipeline:

- Reads exactly one Source.
- Does not browse the Brain.
- Does not decide Let boundaries.
- Does not merge knowledge across Sources.
- Does not reorganize user files.
- Does not generate the final cross-source knowledge experience.

### Pipeline Strategy

Pipelines should be deterministic or heuristic wherever practical.

Bounded LLM calls are appropriate for tasks such as:

- Recognizing that a PDF is a book rather than a generic document.
- Producing a source-local summary.
- Naming sections.
- Describing an image.
- Inferring column semantics from a spreadsheet sample.
- Generating structured metadata under a fixed schema.

An LLM inside Phase 1 remains part of a predefined workflow. It is not an
autonomous workspace agent.

### Pipeline Families

The system should begin with a small number of pipeline families rather than
one implementation per MIME type:

| Family | Typical inputs | Typical outputs |
|---|---|---|
| Document | PDF, DOCX, PPTX, EPUB | Markdown sections, TOC, tables, figures, summary |
| Tabular | CSV, XLSX | Normalized CSV, sheet metadata, profiles, previews |
| Image | PNG, JPEG, HEIC | Normalized image, thumbnail, OCR, caption, metadata |
| Audio | MP3, WAV, M4A | Transcript, subtitles, chapters, highlights, waveform data |
| Video | MP4, MOV, WebM | Transcript, subtitles, chapters, highlights, thumbnails, clips |
| Text/Web | TXT, Markdown, URL snapshot, saved article | Clean text/Markdown, metadata, sections, summary |

A generic fallback pipeline must exist for supported files that cannot be
classified semantically.

### Output Requirements

Pipeline output must:

- Be readable through the shared filesystem.
- Use agent-friendly formats such as Markdown, text, CSV, JSON, YAML, PNG, and
  JPEG.
- Split large content into units an agent can read without context overflow.
- Preserve source order and source locators where relevant.
- Include a manifest that describes the artifact structure and renderer.
- Avoid requiring the agent to parse the original binary.

### Manifest

Every successful pipeline output has one entry manifest:

```json
{
  "source_file_id": "file-123",
  "source_hash": "sha256:...",
  "pipeline": "book",
  "pipeline_version": "1",
  "renderer": "book",
  "title": "Example Book",
  "entrypoints": {
    "summary": "summary.md",
    "toc": "toc.json"
  },
  "artifacts": [
    {
      "path": "chapters/01-introduction.md",
      "role": "chapter",
      "title": "Introduction",
      "order": 1,
      "source_locator": {
        "pages": [1, 18]
      }
    }
  ]
}
```

`pipeline_version` identifies the implementation that produced the current
output. It is not a user-visible content-version history.

### Artifact Persistence

Artifacts are stored in the existing filesystem because both fixed renderers
and agents derive value from them.

They are not merely transient model prompts or search chunks. They form a
reusable, structured representation of the Source.

Temporary OCR buffers, conversion scratch files, and job-local intermediates
may still use ephemeral storage and are not part of the artifact root.

## Phase 1 Renderer Model

Users interact with a Source through a fixed renderer selected by the
pipeline manifest.

### Book Renderer

Consumes:

```text
manifest.json
summary.md
toc.json
chapters/*.md
cover.jpg
figures/*
```

Displays:

- Cover and metadata.
- Table of contents.
- Chapter navigation.
- Source-local summary.
- Search within the book.
- Figures and citations back to page locations.

### Spreadsheet Renderer

Consumes:

```text
manifest.json
workbook.json
sheets/*.csv
profiles/*.json
preview.*
```

Displays:

- Sheet selector.
- Table/grid view.
- Column metadata.
- Basic statistics and data quality information.
- Existing or safely reconstructed charts.

### Video Renderer

Consumes:

```text
manifest.json
subtitles.vtt
transcript.md
chapters.json
highlights.json
thumbnails/*
clips/*
```

Displays:

- Video playback.
- Synchronized subtitles.
- Chapter navigation.
- Transcript search.
- Highlight navigation.

### Audio Renderer

Consumes:

```text
manifest.json
subtitles.vtt
transcript.md
chapters.json
highlights.json
waveform.json
```

Displays:

- Audio playback.
- Synchronized transcript.
- Chapters.
- Highlights.
- Search within the transcript.

### Generic Document Renderer

Provides a safe fallback:

- Original preview.
- Extracted content.
- Outline.
- Tables and figures when available.
- Source-local summary.

## Phase 2 — Brain Integration

### Purpose

Phase 2 converts processed source material into durable, organized knowledge.
It is slower, asynchronous, and aware of the existing Brain.

### Trigger

Phase 2 starts after Phase 1 publishes a ready artifact root:

```text
source.processed
  → enqueue Brain integration
  → mount artifacts read-only
  → mount Brain read/write
  → run Indexer agent
```

Phase 1 readiness must not wait for Phase 2 completion.

### Agent Responsibilities

The Indexer agent:

1. Reads the pipeline manifest.
2. Inspects the artifact structure.
3. Reads artifact files as needed.
4. Inspects the existing Brain tree and relevant Lets.
5. Determines the semantic boundaries represented by the Source.
6. Creates new Lets or updates existing Lets.
7. Splits one Source across several Lets when appropriate.
8. Combines content from several Sources into one Let when appropriate.
9. Writes derived files into the Brain.
10. Records Source-to-Let provenance.

The agent may copy an artifact, edit it, reorganize it, combine it with other
content, or read it and write a new representation. These are ordinary
filesystem operations and do not require a separate promote/transform domain
model.

### Many-to-Many Semantics

The following are all valid:

```text
One Source → no Let yet
One Source → one Let
One Source → several Lets
Several Sources → one Let
Several Sources → several overlapping Lets
```

Examples:

- One PDF containing an unrelated recipe and tax guide may create two Lets.
- Fifteen meeting notes from one sprint may update one Let.
- One financial workbook may update a personal-finance Let, a subscriptions
  Let, and a travel-budget Let.
- Several books may contribute to one topic Let while remaining separate book
  Lets.

## Let Semantics

### Definition

A Let is one coherent unit of knowledge, work, or meaning as determined in the
context of the user's Brain.

Coherence, not size, determines the boundary.

### A Let Is Not

A Let is not:

- A raw upload.
- A search chunk.
- An embedding record.
- A mandatory one-to-one wrapper around a Source.
- Merely a summary.
- Necessarily a single Markdown file.

### Let Contents

A Let may contain any agent-readable files useful for preserving and using
that knowledge:

- Complete normalized chapters or sections.
- Summaries and overviews.
- Full extracted or rewritten text.
- CSV datasets.
- JSON or YAML structured data.
- Images and diagrams.
- Character, entity, or concept indexes.
- Glossaries.
- Timelines.
- Comparisons.
- Analyses and insights.
- Action items.
- Source citations.
- Generative UI specifications.

### Simple Let

```text
/brain/Reference/Cooking/Pad Thai/
├── content.md
└── view.json
```

### Complex Book Let

```text
/brain/Enjoy/Books/Harry Potter 1/
├── overview.md
├── summary.md
├── chapters/
│   ├── 01-the-boy-who-lived.md
│   ├── 02-the-vanishing-glass.md
│   └── ...
├── characters.csv
├── themes.md
├── glossary.md
├── assets/
│   └── cover.jpg
└── view.json
```

### Financial Let

```text
/brain/Finance/Personal Finance 2026/
├── overview.md
├── transactions.csv
├── monthly-summary.json
├── budget-variance.json
├── recurring-expenses.md
├── insights.md
├── charts/
│   └── cash-flow.json
└── view.json
```

## Generative UI for Lets

Phase 2 may create a semantic UI for each Let.

The same source format can result in different Let UIs:

- A finance Let may show metrics, tables, anomalies, and cash-flow charts.
- A research Let may show claims, evidence, citations, and comparisons.
- A project Let may show status, decisions, tasks, and a timeline.
- A book Let may show chapters, characters, themes, and related works.

The product should render Let UI from a controlled component registry rather
than execute arbitrary model-generated frontend code.

Candidate component categories include:

- Text and Markdown.
- Metrics.
- Tables and data grids.
- Charts.
- Timelines.
- Galleries.
- Maps.
- Comparisons.
- Checklists.
- Citations.
- Source previews.

The exact schema and component library remain an implementation decision.

## End-to-End Example — Book PDF

### User Action

The user uploads:

```text
/sources/Books/example-book.pdf
```

The Files UI immediately shows the Source and its processing status.

### Phase 1

The document pipeline:

1. Detects that the PDF is a book.
2. Extracts metadata, cover, text, figures, and table of contents.
3. Splits the book into chapter-sized Markdown files.
4. Creates a source-local summary.
5. Writes the artifact tree.
6. Publishes a `book` manifest.

```text
/artifacts/{source-file-id}/
├── manifest.json
├── summary.md
├── toc.json
├── cover.jpg
├── chapters/
│   ├── 01.md
│   ├── 02.md
│   └── ...
└── figures/
```

The user can now open the Source through `BookView` even while Phase 2 is still
pending.

### Phase 2

The Indexer agent:

1. Reads the manifest and chapters.
2. Searches the Brain for related books, authors, series, and topics.
3. Creates or updates the main book Let.
4. May also update related topic, character, or series Lets.
5. Writes the resulting files into `/brain`.
6. Records which Lets use the Source.

The resulting book Let may preserve all normalized chapters and add new
cross-source analysis. It is not limited to the Phase 1 summary.

## End-to-End Example — Excel Workbook

### User Action

The user uploads:

```text
/sources/Finance/personal-budget-2026.xlsx
```

### Phase 1

The tabular pipeline:

1. Inspects workbook metadata, sheets, tables, formulas, and named ranges.
2. Does not execute untrusted macros.
3. Normalizes useful sheets to CSV.
4. Generates sheet and column profiles.
5. Creates previews and a workbook manifest.

```text
/artifacts/{source-file-id}/
├── manifest.json
├── workbook.json
├── sheets/
│   ├── transactions.csv
│   └── budget.csv
├── profiles/
│   ├── transactions.json
│   └── budget.json
└── preview.png
```

The user can immediately inspect sheets through `SpreadsheetView`.

### Phase 2

The Indexer agent may:

- Update `Personal Finance 2026`.
- Extract recurring payments into a `Subscriptions` Let.
- Add travel-related expenses to a `Japan Trip Budget` Let.
- Run deterministic analysis tools before writing metrics or insights.

The workbook remains one Source while contributing to several Lets.

## UI Information Architecture

A workspace should expose three primary user surfaces:

```text
Files | Brain | Ask
```

### Files

The Source file manager:

- Folder navigation.
- Upload.
- Rename and move.
- Preview.
- Search.
- Download.
- Delete.
- Phase 1 processing status.
- Phase 2 Brain-sync status.
- List of Lets that use the Source.

The MVP should remain a simple file manager. It does not need to reproduce
Google Drive collaboration or office editing.

### Source Detail

Displays the registered Phase 1 renderer rather than the artifact filesystem.

Example status:

```text
File processing: Ready
Brain integration: Processing
Used by: 0 Lets
```

After Phase 2:

```text
File processing: Ready
Brain integration: Integrated
Used by:
  - Personal Finance 2026
  - Subscriptions
```

### Brain

Displays folders and Lets, not raw artifact files.

Opening a Let renders its generative UI and offers navigation to its derived
content and Sources.

### Ask

Provides conversational retrieval and discussion over the Brain. Exact
retrieval/index architecture is deferred, but answers must retain provenance
to the underlying Lets and Sources.

## Processing States

Suggested user-visible state progression:

```text
Uploading
  → Queued
  → Processing file
  → File ready
  → Syncing to Brain
  → Integrated
```

Failure domains remain separate:

- Phase 1 failure does not delete or hide the Source.
- Phase 2 failure does not make the Phase 1 Source view unavailable.
- Phase 2 may retry later without requiring the user to upload again.

## Minimal Metadata

The filesystem remains the content store. Only minimal coordination metadata
is required outside it.

For a Source:

```text
source_file_id
workspace_id
processing_status
artifact_root
renderer_type
source_hash
brain_sync_status
```

For Source-to-Let provenance:

```text
let_id
source_file_id
```

Optional source locators may identify pages, sheet ranges, timestamps, or
sections.

The design intentionally does not require:

- `volume_id`.
- Separate Source, Artifact, and Brain folder tables.
- A user-visible processing-run entity.
- Persistent `SourceVersion` or `ArtifactSet` history.
- Artifact promotion or transformation records.

Operational queue jobs may still have ephemeral job IDs for retries,
observability, and logs.

## Reprocessing Without a Version Domain

The current foundation retains one active artifact root per Source.

A safe reprocessing strategy is:

1. Write new artifacts to an internal temporary location.
2. Validate the output and manifest.
3. Update the Source's `artifact_root` to the completed output.
4. Garbage-collect the previous output asynchronously.

Users and agents see only the current artifact root.

Historical restoration and comparison are deferred.

## Relationship to the Current Prototypes

The architecture is a product foundation, not a requirement to preserve
current prototype boundaries.

The existing projects provide useful building blocks:

### Openlet Backend

Potential ownership:

- Workspace lifecycle and membership.
- Shared file/folder implementation.
- Source upload and user file APIs.
- Object storage.
- Phase 1 processing workers.
- Source view API.
- Processing events and background queues.

### Leti Runtime

Potential ownership:

- Phase 2 agent loop.
- Artifact read mount.
- Brain read/write mount.
- Tool execution.
- Background task lifecycle.
- Permissions at the agent-tool boundary.
- Let creation and update behavior.

### Frontend

Potential ownership:

- Files explorer.
- Fixed Source renderers.
- Brain tree.
- Let generative renderer.
- Ask experience.
- Processing and sync status.

The current Python and Rust agent implementations are prototypes. Choosing the
long-term runtime owner is a separate architecture decision.

## Functional Requirements

| ID | Requirement | Priority |
|---|---|---|
| F1 | Create `/sources`, `/artifacts`, and `/brain` for every workspace. | Must |
| F2 | Prevent ordinary principals from renaming, moving, or deleting reserved roots. | Must |
| F3 | Allow users to manage files and folders under `/sources`. | Must |
| F4 | Route every uploaded Source through a Phase 1 pipeline. | Must |
| F5 | Store Phase 1 output under `/artifacts/{source-file-id}`. | Must |
| F6 | Require a manifest for every ready artifact root. | Must |
| F7 | Render a processed Source through a registered fixed renderer. | Must |
| F8 | Prevent users from browsing or mutating raw artifact files directly. | Must |
| F9 | Allow the Phase 2 agent to read artifacts without reading the Source tree. | Must |
| F10 | Allow the Phase 2 agent to read and write the Brain. | Must |
| F11 | Run Phase 2 asynchronously after Phase 1 becomes ready. | Must |
| F12 | Support many-to-many Source–Let relationships. | Must |
| F13 | Allow a Let to contain multiple derived files and nested directories. | Must |
| F14 | Render Lets through a generative UI layer. | Should |
| F15 | Display Source processing and Brain-sync status. | Must |
| F16 | Show which Lets use a Source. | Should |
| F17 | Preserve access to a Source when either processing phase fails. | Must |

## Non-Functional Requirements

| ID | Requirement | Initial metric |
|---|---|---|
| NF1 | Namespace isolation | No scoped API may resolve a path outside its reserved root. |
| NF2 | Tenant isolation | Every filesystem request is scoped to one authenticated workspace. |
| NF3 | Phase isolation | Phase 2 failure cannot invalidate Phase 1 output. |
| NF4 | Agent context safety | Pipelines split large content into bounded, independently readable artifacts. |
| NF5 | Retry safety | Repeating a Phase 1 job must not expose partially written output as ready. |
| NF6 | Traceability | A Let can identify the Sources that contributed to it. |
| NF7 | Portability | Core textual/tabular artifacts use open formats such as Markdown, text, CSV, JSON, and YAML. |
| NF8 | UI safety | Source renderers and Let components use registered frontend components, not arbitrary generated code execution. |

## Explicit Non-Goals

The current foundation does not include:

- Multiple user-visible workspace types.
- Folder-level ACL.
- A complete Google Drive replacement.
- Collaborative document editing.
- User browsing of raw pipeline artifact trees.
- Agent access to arbitrary raw Source folders.
- One Let per upload.
- One Source per Let.
- A requirement that a Let contain only a summary.
- Autonomous Phase 1 organization of the Brain.
- Generative UI for individual Sources.
- Persistent source or artifact version history.
- Final HITL and approval rules.
- Final deletion/cascade policy.

## Deferred Decisions

The following require later product or architecture decisions:

1. User mutation permissions inside the Brain.
2. HITL thresholds and confirmation policy for agent operations.
3. Undo, history, revisions, and restore.
4. Source deletion behavior when Lets depend on it.
5. Artifact retention and cleanup after Source deletion.
6. Cross-workspace Source and Let relationships.
7. Whether Ask searches only the Brain or may also inspect Phase 1 artifacts.
8. Full-text, embedding, hybrid, and graph retrieval strategy.
9. Top-level Brain taxonomy.
10. Let metadata persistence and exact manifest format.
11. Deep-analysis scheduling, cost budgets, and latency targets.
12. The final generative UI schema and component registry.
13. The long-term ownership boundary between the Python and Rust agent
    prototypes.
14. Sharing and collaboration beyond workspace-level roles.

## Acceptance Criteria

- [ ] Creating a workspace creates the three reserved roots.
- [ ] Source, Artifact, and Brain APIs reuse the same filesystem
      implementation.
- [ ] A user can create folders and manage files under `/sources`.
- [ ] A user cannot traverse into `/artifacts` through the Files API.
- [ ] A pipeline can read one Source and write its artifact root.
- [ ] A fixed renderer can display a processed Source without exposing raw
      artifact browsing.
- [ ] An Indexer agent can mount relevant artifacts read-only and the Brain
      read/write.
- [ ] The Indexer agent cannot access `/sources` through its scoped mount.
- [ ] A book pipeline can produce chapters and a TOC that power a fixed book
      reader.
- [ ] A spreadsheet pipeline can produce normalized sheets that power a fixed
      spreadsheet viewer.
- [ ] Phase 1 readiness is available before Phase 2 completes.
- [ ] One Source can be linked to multiple Lets.
- [ ] One Let can be linked to multiple Sources.
- [ ] A complex Let can store complete derived chapters, datasets, images, and
      analyses rather than only a summary.
- [ ] A Phase 2 failure leaves the Source and its Phase 1 UI usable.

## Next Steps

1. Treat this document as the product and architecture foundation for Let.
2. Convert the confirmed decisions into an implementation plan only after the
   deferred permission and deletion questions become relevant.
3. Define the Phase 1 manifest contract and the first two renderers.
4. Implement one vertical slice for a book PDF and one for an Excel workbook.
5. Validate that Leti can consume artifacts through a read-only mount and write
   a complete Let through a Brain mount.
6. Test the two phases independently before optimizing retrieval or adding
   advanced generative UI.

## References

- `drafts/draft-1.md` through `drafts/draft-5.md`
- `drafts/techstack.md`
- `drafts/ui-rules.md`
- `docs/system-architecture.md`
- `docs/project-overview-pdr.md`
- `../openlet/docs/system-architecture.md`
- `../openlet/docs/codebase-summary.md`
