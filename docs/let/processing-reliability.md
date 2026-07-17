# Processing Reliability

## Overview

Processing is asynchronous and has two independent failure domains. The system
must preserve user Sources, publish only complete Artifact Roots, and allow
Brain integration to retry without forcing another upload.

## State Model

The product-level Source processing state is:

```text
Uploading → Queued → Processing file → File ready → Syncing to Brain → Integrated
```

Implementations may track more detailed operational states, retries, error
codes, and queue job IDs. Those are internal coordination data, not durable
user-content objects.

## Phase Isolation

| Failure | Required outcome |
|---|---|
| Upload fails | No incomplete Source is presented as ready |
| Phase 1 fails | Source remains visible; user can retry or inspect failure state |
| Fixed renderer fails | Generic fallback or recoverable error; Source remains accessible |
| Phase 2 fails | Ready Source and Phase 1 view remain available; Brain job may retry |
| Let rendering fails | Derived files and provenance remain accessible through a safe fallback |

Phase 2 failure MUST NOT invalidate Phase 1 output. Phase 1 success MUST NOT
be blocked by an unavailable agent runtime.

## Atomic Artifact Publication

The system retains one active Artifact Root per Source. A safe Phase 1 publish
sequence is:

1. Write pipeline output to an internal temporary location.
2. Validate output, required files, and manifest.
3. Atomically expose the completed output as the Source's active Artifact Root.
4. Publish `source.processed` only after the active root is valid.
5. Garbage-collect superseded temporary or prior output asynchronously.

No renderer or agent may read a partially published Artifact Root.

## Reprocessing Without a Version Domain

The foundation does not introduce persistent `SourceVersion`, `ArtifactSet`, or
user-visible processing-run history. Reprocessing replaces the active artifact
representation after successful validation. `pipeline_version` records which
implementation made current output; it does not create a content-history API.

Historical restore, visual diff, and audit retention are deferred decisions.
Operational logs and ephemeral job IDs may exist for observability and retries.

## Idempotency and Retry

Phase 1 and Phase 2 work MUST be retry-safe:

- Repeating a pipeline job cannot expose a partial root as ready.
- A duplicate integration trigger must not blindly duplicate the same derived
  files, provenance links, or Lets.
- The agent must read the current Let scope and provenance before writing.
- Failed jobs must retain enough input identity to retry against the correct
  Source hash and active artifact root.

The precise idempotency key is an implementation concern, but it should include
Workspace, Source identity, and the active processing representation rather
than a mutable user filename.

## Minimal Coordination Metadata

The filesystem stores content. The system needs only minimal metadata outside
the tree:

```text
Source:
  source_file_id
  workspace_id
  processing_status
  artifact_root
  renderer_type
  source_hash
  brain_sync_status

Provenance:
  let_id
  source_file_id
  optional source locator
```

The model intentionally does not require `volume_id`, separate Source/Artifact/
Brain file tables, user-visible run entities, persistent version histories, or
artifact-promotion records.

## Observability

Operators need event correlation from upload through Phase 1 and Phase 2. Logs
and metrics SHOULD include Workspace ID, Source ID, artifact representation,
pipeline family/version, job ID, and affected Let IDs. Logs must avoid leaking
unnecessary source content or sensitive derived content.

## Deferred Reliability Policies

Source deletion, artifact retention after deletion, Brain rollback, revision
history, undo, conflict resolution, and user approval of destructive agent
operations remain unresolved. Do not smuggle such policy into retry code.

## References

- [Source Processing](./source-processing.md)
- [Brain Integration](./brain-integration.md)
- [Requirements](./requirements.md)
- [Decisions and Open Questions](./decisions-and-open-questions.md)
