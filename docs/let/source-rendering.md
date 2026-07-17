# Source Rendering

## Overview

Phase 1 gives each processed Source a fixed, type-specific interface. This UI
is selected by the Artifact Root manifest and is deliberately separate from
generative Let UI.

The renderer exposes the value of the current Source without exposing its raw
Artifact filesystem. It must be stable, testable, accessible, and safe to
render before Phase 2 has completed.

## Renderer Selection

The Source View service reads `manifest.json`, validates the declared renderer,
and chooses a registered implementation.

```text
Source ID → active Artifact Root → manifest.renderer → registered Source view
```

A pipeline cannot cause arbitrary frontend code to execute by writing a
renderer name or view file. If the selected renderer is unavailable or required
inputs are missing, the service MUST fall back to a safe generic document view
and surface a recoverable processing error.

## Renderer Contract

Each renderer MUST:

- Consume only documented artifact paths from the manifest.
- Render a safe view model, not arbitrary HTML or JavaScript from artifacts.
- Preserve navigation to relevant Source locations where possible.
- Handle missing optional artifacts gracefully.
- Work without access to the Brain.
- Show processing state and a route back to the original Source.

Each renderer SHOULD support search, navigation, and progressive loading for
large Sources. It MAY expose source-local summaries and analysis produced by
the pipeline, but those are not Lets and do not define Brain structure.

## Initial Renderer Families

### Book Renderer

The book renderer consumes `manifest.json`, `summary.md`, `toc.json`, chapter
Markdown, optional `cover.jpg`, and figures. It displays cover and metadata,
table of contents, chapter navigation, source-local summary, in-book search,
figures, and citations back to pages.

### Spreadsheet Renderer

The spreadsheet renderer consumes workbook metadata, normalized sheet CSVs,
column profiles, and previews. It displays a sheet selector, grid/table view,
column metadata, basic statistics, data-quality information, and existing or
safely reconstructed charts. It MUST NOT execute untrusted workbook macros or
formulas in a privileged environment.

### Video Renderer

The video renderer consumes subtitle, transcript, chapter, highlight,
thumbnail, and optional clip artifacts. It displays playback, synchronized
subtitles, chapter navigation, transcript search, and highlights.

### Audio Renderer

The audio renderer consumes subtitles, transcript, chapters, highlights, and
waveform data. It displays playback, synchronized transcript, chapter and
highlight navigation, and transcript search.

### Generic Document Renderer

The fallback renderer presents original preview, extracted content, outline,
tables and figures when available, and a source-local summary. It prevents a
classification miss from making the Source unusable.

## Source View Is Not a Let View

| Dimension | Source View | Let View |
|---|---|---|
| Input | One Artifact Root | One Let's derived files and metadata |
| UI selection | Fixed by pipeline/renderer family | Controlled semantic composition |
| Knowledge scope | One uploaded Source | One semantic responsibility; may use many Sources |
| Availability | After Phase 1 | After Phase 2 creates or updates a Let |
| User navigation | Files → Source detail | Brain → Let detail |

Do not solve a weak source renderer by creating a Let early. Phase 1 and Phase
2 have separate product value and separate failure domains.

## References

- [Source Processing](./source-processing.md)
- [Let Model](./let-model.md)
- [User Experience](./user-experience.md)
- [Access Control and APIs](./access-control-and-apis.md)
