# Brain Integration

## Overview

Phase 2 converts processed Source material into durable, organized knowledge.
It is asynchronous, agent-driven, and aware of the existing Brain. Its role is
not to repeat extraction; it reads structured artifacts and decides how their
knowledge should be represented for future retrieval and use.

## Trigger and Isolation

Phase 2 begins after Phase 1 has atomically published a ready Artifact Root:

```text
source.processed
  → enqueue Brain integration
  → mount selected artifacts read-only
  → mount Brain read/write
  → run Indexer agent
```

Phase 1 readiness MUST NOT wait for Phase 2. A Phase 2 failure leaves the
Source, its Artifact Root, and its fixed source view available.

## Agent Inputs

For a single integration job, the agent receives:

- The Source identity and processing metadata.
- The Artifact Root manifest.
- The relevant Artifact files, read-only.
- The existing Brain tree and candidate Lets, read/write through `/brain`.
- A scoped toolset for reading, writing, moving, and recording provenance.

The agent does not receive the raw Source tree by default. This prevents
re-parsing binaries, limits context load, and makes Phase 2 depend on the
structured contract produced by Phase 1.

## Responsibilities

The Indexer agent MUST:

1. Read the manifest before consuming artifacts.
2. Inspect artifact structure and load content only as needed.
3. Inspect existing Brain folders and candidate Lets before writing.
4. Determine the semantic responsibilities represented by the new material.
5. Apply the Let Boundary rules to create, update, split, merge, or link.
6. Write the required derived files into the Brain.
7. Record Source-to-Let provenance, including locators when available.
8. Preserve enough material in Lets for their stated scope; it must not reduce
   every artifact to a summary.

The agent MAY copy an artifact unchanged, lightly edit it, reorganize it,
combine it with other content, or read it and write a new representation. These
are filesystem operations, not a separate promotion or transform workflow.

## Completeness Requirement

Artifacts are the pipeline's digestible representation of a Source. If the
agent uses an artifact to support a Let, the resulting Brain state MUST retain
the information necessary for that Let's declared scope. The agent may retain a
copy, a normalized form, a structured interpretation, or a combination of
derived files; the correct choice is content-specific.

Phase 1 retains the full Artifact Root regardless, so not every pipeline file
must be duplicated into every Let. The key rule is that a Let must remain useful
and defensible for its own scope without relying on an undocumented hidden
interpretation.

## Many-to-Many Semantics

The following are normal, not exceptional:

```text
One Source → no Let yet
One Source → one Let
One Source → several Lets
Several Sources → one Let
Several Sources → several overlapping Lets
```

Examples of the boundary logic:

- A Source that contains unrelated, independently useful responsibilities may
  update or create separate Lets.
- Many small Sources that contribute to one stable responsibility should update
  one Let rather than create one Let per upload.
- A Source may update a source-oriented Let and also contribute evidence to a
  topic-oriented Let.

Shared provenance does not require merging. Related but independently useful
Lets should be linked, not collapsed into one broad document.

## Integration Workflow

```text
Read manifest
  → identify candidate knowledge responsibilities
  → retrieve candidate Lets by scope and provenance
  → decide update/create/link
  → write derived content and provenance
  → validate Let boundary
  → publish integration result
```

Split and merge are not default ingestion actions. They are high-impact
maintenance actions and require stronger evidence than a normal update. When
uncertain, the agent SHOULD preserve existing stable boundaries and link related
Lets rather than oscillate between split and merge.

## Provenance Requirements

Each material Source contribution requires a Source-to-Let relation. The
relation SHOULD capture the narrowest locator available: page range, chapter,
sheet and range, media timestamp, section, or artifact path. A Let view should
surface the supporting Sources, and a Source detail should list Lets that use
it.

Provenance is not just citation UI. It is the system's basis for trust,
reprocessing, future deletion policy, and conflict investigation.

## Explicit Limits

The agent is not responsible for Phase 1 parsing, renderer selection, user
source-folder organization, arbitrary raw-source access, or the final Ask
retrieval architecture. Those are separate boundaries.

## References

- [Let Boundary](./let-boundary.md)
- [Let Model](./let-model.md)
- [Source Processing](./source-processing.md)
- [Processing Reliability](./processing-reliability.md)
