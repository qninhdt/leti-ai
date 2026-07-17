# End-to-End Scenarios

## Overview

These scenarios show the intended model without making the model depend on any
one file type. The same rules apply to all Sources: Phase 1 digests one Source,
Phase 2 integrates the resulting artifacts into the Brain according to Let
scope and lifecycle.

## Generic Lifecycle

```text
User uploads Source to /sources
  → system records Source and queues Phase 1
  → pipeline writes and publishes Artifact Root
  → fixed Source renderer becomes available
  → integration job mounts artifacts and Brain
  → agent updates, creates, links, splits, or merges Lets
  → provenance and Brain-sync status are published
```

At any point after upload, the Source remains the user's original file. At any
point after Phase 1, the Source renderer remains usable even if Brain work is
pending or fails.

## Book PDF

### Source and Phase 1

The user uploads:

```text
/sources/Books/example-book.pdf
```

The document pipeline identifies a book, extracts metadata, cover, text,
figures, and table of contents, splits text into chapters, and writes a
source-local summary.

```text
/artifacts/{source-file-id}/
├── manifest.json
├── summary.md
├── toc.json
├── cover.jpg
├── chapters/
│   ├── 01.md
│   └── 02.md
└── figures/
```

The user can immediately open the Source in a book renderer.

### Phase 2

The agent reads the manifest and selected chapters, searches existing Brain
scope contracts, and may create or update a source-oriented book Let. It may
also add evidence or content to existing topic, character, author, or series
Lets when those are independently useful responsibilities.

The book Source does not automatically require one book Let per chapter. A
chapter becomes its own Let only if it has an independently useful scope and
lifecycle. Otherwise, chapters are derived files inside the book Let or are
used as evidence in another Let.

## Excel Workbook

### Source and Phase 1

The user uploads:

```text
/sources/Finance/personal-budget-2026.xlsx
```

The tabular pipeline inspects workbook metadata, sheets, tables, formulas, and
named ranges; does not execute untrusted macros; normalizes useful sheets to
CSV; creates profiles and previews; and writes a workbook manifest.

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

The user can immediately inspect sheets through the spreadsheet renderer.

### Phase 2

The agent may update a Let whose scope is personal financial position, create a
separate subscriptions Let if recurring-payment analysis is independently useful,
and link both. It may also add selected evidence to a trip-budget Let. The
workbook remains one Source while contributing to several Lets.

## Small Note

A two-sentence note may become a Let if it is a complete, durable, directly
addressable idea. If it is only a fragment of an existing plan or project, the
agent updates that existing Let instead. Shortness is not a reason to reject a
Let; absence of independent identity is.

## Broad Mixed Source

If one document contains two unrelated responsibilities, the agent should not
force them into one Let because they arrived in one file. It may create two
Lets, retain provenance to the same Source with different locators, and place
them in separate Brain folders.

## Multi-Source Synthesis

Many Sources can update one Let when they jointly serve a single scope and are
normally read or maintained together. The Let records each contributing Source
and relevant locators. The agent must not create one Let per Source merely to
preserve provenance; provenance is a relation, not an ownership constraint.

## References

- [Source Processing](./source-processing.md)
- [Brain Integration](./brain-integration.md)
- [Let Boundary](./let-boundary.md)
- [User Experience](./user-experience.md)
