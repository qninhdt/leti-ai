# Requirements

## Overview

These requirements define the minimum implementation behavior for the Openlet
knowledge system. They describe the foundation, not every future feature.

## Functional Requirements

| ID | Requirement | Priority |
|---|---|---|
| F1 | Create `/sources`, `/artifacts`, and `/brain` for every Workspace. | Must |
| F2 | Prevent ordinary principals from renaming, moving, or deleting reserved roots. | Must |
| F3 | Allow users to manage files and folders under `/sources`. | Must |
| F4 | Route each uploaded Source through a Phase 1 pipeline. | Must |
| F5 | Store ready Phase 1 output under `/artifacts/{source-file-id}`. | Must |
| F6 | Require a valid manifest for every ready Artifact Root. | Must |
| F7 | Render a processed Source through a registered fixed renderer. | Must |
| F8 | Prevent users from browsing or mutating raw artifact files directly. | Must |
| F9 | Allow Phase 2 to read assigned Artifacts without raw Source-tree access. | Must |
| F10 | Allow Phase 2 to read and write the Brain. | Must |
| F11 | Run Phase 2 asynchronously after Phase 1 is ready. | Must |
| F12 | Support many-to-many Source–Let provenance. | Must |
| F13 | Allow a Let to contain multiple derived files and nested directories. | Must |
| F14 | Persist a stable title and scope statement for every Let. | Must |
| F15 | Apply create, update, split, merge, and link using the Let Boundary contract. | Must |
| F16 | Render Lets through a controlled generative UI layer. | Should |
| F17 | Display Source processing and Brain-sync status. | Must |
| F18 | Show Lets that use a Source and Sources that support a Let. | Should |
| F19 | Preserve Source access when either processing phase fails. | Must |

## Non-Functional Requirements

| ID | Requirement | Initial metric or invariant |
|---|---|---|
| NF1 | Namespace isolation | No scoped API resolves outside its reserved root. |
| NF2 | Tenant isolation | Every filesystem request is authenticated and scoped to one Workspace. |
| NF3 | Phase isolation | Phase 2 failure cannot invalidate a ready Phase 1 output. |
| NF4 | Agent context safety | Pipelines decompose large content into bounded readable artifacts. |
| NF5 | Publish safety | No partially written Artifact Root is observable as ready. |
| NF6 | Retry safety | Duplicate jobs do not blindly duplicate Lets or provenance. |
| NF7 | Traceability | A Let identifies contributing Sources and locators when available. |
| NF8 | Portability | Core textual/tabular artifacts use open formats such as Markdown, text, CSV, JSON, and YAML. |
| NF9 | UI safety | Renderer components are registered and validated; no arbitrary generated code runs. |
| NF10 | Boundary stability | Repeated integration does not churn Let identity without a material scope/lifecycle reason. |

## Acceptance Criteria

- [ ] Creating a Workspace creates the three reserved roots.
- [ ] Source, Artifact, and Brain APIs reuse one filesystem implementation.
- [ ] A user can manage Sources but cannot traverse into `/artifacts` through
      the Files API.
- [ ] A pipeline can read one Source and write only its Artifact Root.
- [ ] A fixed renderer displays a processed Source without exposing raw
      artifacts.
- [ ] An Indexer agent mounts selected Artifacts read-only and Brain read/write.
- [ ] The agent cannot access `/sources` through its scoped mount.
- [ ] A document pipeline can produce chapters and TOC for a fixed book reader.
- [ ] A tabular pipeline can produce normalized sheets for a spreadsheet view.
- [ ] Phase 1 is available before Phase 2 completes.
- [ ] One Source can link to multiple Lets and one Let to multiple Sources.
- [ ] A Let can retain chapters, data, images, and analyses, not only a summary.
- [ ] Every Let has a stable scope statement.
- [ ] Related Lets can remain separate and linked.
- [ ] A Phase 2 failure leaves Source and Phase 1 UI usable.

## References

- [Decisions and Open Questions](./decisions-and-open-questions.md)
- [Let Boundary](./let-boundary.md)
- [Processing Reliability](./processing-reliability.md)
