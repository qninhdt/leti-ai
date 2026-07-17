# Let Model

## Overview

A Let is the primary user-visible unit in the Brain. It is a logical derived
document, not necessarily a single physical file. The agent may represent a
Let as one Markdown file or as a directory with text, tables, images, indexes,
and view data.

## Canonical Definition

> A Let is the smallest user-addressable derived document that is independently
> useful, has one stable semantic scope, and is maintained as one unit.

This definition deliberately does not mention source count, file type, token
count, byte size, or summary length. Those characteristics vary by content; the
boundary is semantic and operational.

## What a Let Is Not

A Let is not:

- A raw upload.
- A search chunk or an embedding record.
- A mandatory one-to-one wrapper around a Source.
- A summary-only document.
- A single file by definition.
- A Folder that collects several unrelated but broadly themed items.

## Let Anatomy

Every Let has a stable identity, a human-readable title, a scope statement,
derived content, and provenance. The exact on-disk shape may vary, but a
directory representation illustrates the logical parts:

```text
/brain/{path-to-let}/
├── let.json                 # identity, title, scope, display metadata
├── overview.md              # entry point for people and agents
├── content/                 # optional text, tables, structured data
├── assets/                  # optional images or derived media
├── views/                   # optional declarative generative UI data
└── provenance.json          # Source links and locators
```

`let.json`, `overview.md`, and `provenance.json` describe the logical contract;
their exact serialized shape is an implementation detail. A simple Let may be
only `content.md` plus minimal metadata. The system MUST NOT force complex
content into an artificial universal directory shape.

## Content Policy

Lets may contain whatever agent-readable files are needed to preserve and use
their scope, including:

- Complete normalized chapters or sections.
- Full extracted or rewritten text.
- Summaries, overviews, and indexes.
- CSV datasets and normalized tables.
- JSON or YAML structured data.
- Images, diagrams, maps, and charts.
- Entity or concept indexes, glossaries, timelines, and comparisons.
- Analysis, insight, action items, and citations.

The presence of an overview does not turn a Let into a summary. The overview
is an entrypoint; the Let may retain substantial underlying content.

## Examples of Representation

### Simple Let

```text
/brain/Reference/Cooking/Pad Thai/
├── content.md
└── let.json
```

### Source-Oriented Book Let

```text
/brain/Enjoy/Books/Example Book/
├── overview.md
├── chapters/
│   ├── 01.md
│   └── 02.md
├── characters.csv
├── themes.md
├── glossary.md
├── assets/cover.jpg
└── provenance.json
```

### Analytical Let

```text
/brain/Finance/Personal Finance 2026/
├── overview.md
├── transactions.csv
├── monthly-summary.json
├── recurring-expenses.md
├── insights.md
├── charts/cash-flow.json
└── provenance.json
```

## Generative Let UI

Lets may render through a semantic UI assembled from a controlled component
registry. The same Source format can therefore yield different Let interfaces:

- Finance: metrics, tables, anomalies, and cash-flow charts.
- Research: claims, evidence, citations, and comparisons.
- Project: status, decisions, tasks, and timeline.
- Book: chapters, characters, themes, and related works.

The agent may propose declarative view data, but the frontend MUST render only
registered components and validated schemas. It must never execute arbitrary
model-generated frontend code.

## Let Versus Folder

| Question | Let | Folder |
|---|---|---|
| Has a stable semantic scope? | Yes, exactly one | May be broad |
| Is useful when opened directly? | Yes | Not required |
| Has a direct retrieval identity? | Yes | Usually navigational |
| Has one maintenance lifecycle? | Yes | No requirement |
| Contains several independent Lets? | No | Yes |

## Let Versus Source

A Source answers “what did the user upload?” A Let answers “what derived body
of knowledge does the user now have?” One Source may support many answers; many
Sources may support one answer.

## Required Metadata

At minimum, each Let needs stable `let_id`, title, scope statement, path, and
provenance references. The agent must use the stored scope statement on later
runs rather than reconstruct the boundary from title or embeddings alone.

## References

- [Let Boundary](./let-boundary.md)
- [Brain Integration](./brain-integration.md)
- [User Experience](./user-experience.md)
- [Domain Model](./domain-model.md)
