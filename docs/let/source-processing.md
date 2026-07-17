# Source Processing

## Overview

Phase 1, File Digestion, makes one Source understandable and usable without
awareness of any other Source or Let. It performs specialized work that an
agent should not repeat: extraction, normalization, decomposition, previews,
and source-local metadata.

It writes a durable Artifact Root under `/artifacts/{source-file-id}` and
publishes a manifest consumed by both fixed source renderers and Phase 2.

## Scope Boundary

A Phase 1 pipeline MUST:

- Read exactly one Source.
- Write only that Source's assigned Artifact Root.
- Preserve source order and source locators when relevant.
- Produce agent-readable files and renderer input.

A Phase 1 pipeline MUST NOT:

- Browse or modify the Brain.
- Decide Let boundaries.
- Merge knowledge across Sources.
- Organize user folders or rename user files.
- Produce the final cross-source knowledge experience.

## Processing Strategy

Use deterministic code, heuristics, and specialized parsers wherever practical.
Bounded LLM calls are acceptable inside a predefined workflow for tasks such
as semantic classification, section naming, source-local summaries, image
captions, or column-semantic inference under a fixed schema.

An LLM in Phase 1 is not an autonomous agent: it has one Source, bounded input
and output schemas, no Brain mount, and no authority to make workspace-level
decisions.

## Pipeline Families

| Family | Typical inputs | Typical durable outputs |
|---|---|---|
| Document | PDF, DOCX, PPTX, EPUB | Markdown sections, TOC, tables, figures, summary |
| Tabular | CSV, XLSX | Normalized CSV, sheet metadata, profiles, previews |
| Image | PNG, JPEG, HEIC | Normalized image, thumbnail, OCR, caption, metadata |
| Audio | MP3, WAV, M4A | Transcript, subtitles, chapters, highlights, waveform data |
| Video | MP4, MOV, WebM | Transcript, subtitles, chapters, highlights, thumbnails, clips |
| Text/Web | TXT, Markdown, URL snapshot, saved article | Clean text/Markdown, metadata, sections, summary |

The system MUST provide a generic fallback for supported files that cannot be
classified semantically. Adding a new MIME type should normally extend a family
instead of creating an entirely new processing architecture.

## Output Requirements

Pipeline output MUST:

- Be readable through the shared filesystem.
- Prefer portable formats: Markdown, text, CSV, JSON, YAML, PNG, and JPEG.
- Split large text or tables into bounded units that an agent can read without
  context overflow.
- Preserve source order and locators such as page, section, row range, or
  timestamp where applicable.
- Include a complete manifest.
- Avoid requiring Phase 2 to parse the original binary again.

The artifact layout is intentionally pipeline-specific. A folder hierarchy is
preferred when it makes renderer and agent navigation clearer; artifacts do not
need to be a flat list.

## Manifest Contract

Every ready Artifact Root MUST contain `manifest.json` at its root. The exact
schema may evolve, but it must identify source, pipeline, renderer, entrypoints,
and artifact files.

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
      "source_locator": { "pages": [1, 18] }
    }
  ]
}
```

`pipeline_version` identifies the implementation that produced current output.
It is not a user-visible history or a Source-version domain model.

## Artifact Persistence

Artifacts are reusable structured representations, not merely transient model
prompts or search chunks. Fixed renderers and the agent both derive value from
them, so the active Artifact Root is retained in the shared filesystem.

Temporary OCR buffers, conversion scratch files, and job-local intermediates
MAY use ephemeral storage and MUST NOT be mistaken for durable artifacts.

## Completion Contract

A pipeline may publish `File ready` only after all required output is written,
the manifest is valid, renderer entrypoints exist, and the active Artifact Root
switch is atomic. Reliability details are in
[Processing Reliability](./processing-reliability.md).

## References

- [Source Rendering](./source-rendering.md)
- [Brain Integration](./brain-integration.md)
- [Workspace Filesystem](./workspace-filesystem.md)
- [Processing Reliability](./processing-reliability.md)
