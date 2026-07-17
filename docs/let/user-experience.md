# User Experience

## Overview

The product exposes one Workspace through three primary surfaces:

```text
Files | Brain | Ask
```

The surfaces intentionally show different representations of the same long-term
data system. Files is for original user assets; Brain is for agent-managed
derived knowledge; Ask is for conversational retrieval. Users must never need
to browse raw artifacts to get value from processing.

## Files

Files is a conventional Source explorer. It MUST support folder navigation,
upload, rename, move, preview, search, download, delete, and visible processing
status. It should remain a simple file manager in the MVP rather than recreate
every collaboration or office-editing feature of Google Drive.

The presence of Brain integration must not remove user control over original
file placement. A user may organize Sources manually while pipelines and the
agent process them in the background.

## Source Detail

Opening a processed Source invokes its registered fixed renderer, not a raw
artifact browser. Source detail should show:

- Original file identity and location.
- Phase 1 file-processing status.
- Fixed type-specific content view.
- Phase 2 Brain-sync status.
- Lets that use the Source after integration.
- A route to download or otherwise inspect the original according to role.

Example state before Brain integration:

```text
File processing: Ready
Brain integration: Processing
Used by: 0 Lets
```

Example state after integration:

```text
File processing: Ready
Brain integration: Integrated
Used by:
  - Personal Finance 2026
  - Subscriptions
```

The Source remains useful when Brain integration is pending or failed.

## Brain

Brain displays Folders and Lets, not artifact files. A Folder supports broad
navigation; a Let represents one independently useful semantic responsibility.
Opening a Let renders its overview, derived content, registered UI components,
and supporting Sources.

The Brain experience should make the agent's work legible:

- Let title and stated scope.
- Content navigation appropriate to the Let.
- Source provenance and locators.
- Related Lets when available.
- Last integration/update state when it helps trust.

The initial foundation only guarantees user read access in Brain. The exact
manual mutation model is deferred; the UI must not imply unimplemented direct
editing semantics.

## Let View

A Let view is a controlled semantic composition, not an arbitrary webpage the
model generated. It can combine Markdown, tables, metrics, charts, timelines,
galleries, comparisons, checklists, citations, and source previews from a
registered component library.

The view must always provide a comprehensible default entrypoint. Generative UI
is an enhancement over derived files, not the only way to access a Let.

## Ask

Ask provides conversational retrieval and discussion over the Brain. The exact
retrieval architecture is deferred, but each answer SHOULD retain provenance to
the underlying Lets and Sources. Ask should not silently treat raw artifacts as
the same trust level or product boundary as Brain knowledge.

## Processing States

The initial user-visible lifecycle is:

```text
Uploading
  → Queued
  → Processing file
  → File ready
  → Syncing to Brain
  → Integrated
```

Failure is phase-specific:

- A Phase 1 failure does not delete or hide the Source.
- A Phase 2 failure does not remove a ready source view.
- Retrying Phase 2 does not require the user to upload the file again.

## UX Invariants

1. Users manage originals in Files, not through hidden agent abstractions.
2. Users consume artifacts through Source renderers, not an internal artifact
   file tree.
3. Users consume derived knowledge through Brain and Lets.
4. The UI shows that Source and Brain completion are separate states.
5. Every displayed Let can lead to its supporting Sources.
6. Every displayed Source can lead to Lets that materially use it once known.

## References

- [Source Rendering](./source-rendering.md)
- [Let Model](./let-model.md)
- [Processing Reliability](./processing-reliability.md)
- [Access Control and APIs](./access-control-and-apis.md)
