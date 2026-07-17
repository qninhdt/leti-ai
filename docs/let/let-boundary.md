# Let Boundary

## Overview

“Coherent knowledge” alone is too vague for an agent to use consistently. This
document turns that intuition into a stable boundary contract and explicit
rules for creating, updating, splitting, merging, and linking Lets.

The boundary is deliberately general. It does not depend on whether the input
was a PDF, workbook, video, note, image, or a collection of Sources.

## Boundary Definition

> A Let is the smallest user-addressable derived document that is independently
> useful, has one stable semantic scope, and is maintained as one unit.

The boundary has three inseparable dimensions:

| Dimension | Question | Consequence |
|---|---|---|
| Scope | What single body of knowledge and purpose does this Let own? | Different responsibility means a different Let or Folder |
| Independent utility | Is it useful when directly opened, retrieved, or discussed? | A fragment that is not useful alone stays inside a Let |
| Lifecycle | Will its content normally be updated, checked, and presented together? | Material with a separate lifecycle should be separate |

The memorable operational rule is:

> **One purpose, one entry point, one lifecycle.**

## Let Scope Contract

Every Let MUST persist a short scope statement. It is a contract, not a
generated description that can drift after every ingestion.

```yaml
let_id: let_...
title: Customer Retention Analysis
scope: >
  Explains the factors affecting customer retention and records the evidence,
  analyses, and conclusions used to evaluate them.
```

The title is for navigation. The scope is for boundary decisions. A useful
scope statement says what the Let contains and why a user would open it; it
must not merely repeat source filenames or list unrelated topics.

Optional `includes` and `excludes` fields may be added later if a scope needs
extra precision, but the foundation requires only a stable title and scope.

## Lower Boundary: When Content Is Too Small

Content is not a Let when it lacks independent identity. A fragment should stay
inside an existing Let, or remain in artifacts pending integration, when it:

- Cannot be given a useful direct title.
- Cannot be retrieved or discussed without opening a larger parent Let.
- Contains insufficient context to be useful alone.
- Has no separate expected update or maintenance lifecycle.

A short item can still be a Let. A complete, durable, actionable idea may be a
Let even if it is two sentences long. Length, file count, token count, and byte
size are not lower-bound tests.

## Upper Boundary: When Content Is Too Broad

A Let is too broad when it contains two or more subparts that each satisfy the
Let definition and are only weakly coupled. Split when the subparts can each:

1. Be given a distinct scope statement and title.
2. Be retrieved or opened independently by a plausible future user request.
3. Be updated, verified, or rendered without requiring the other subparts.

If a broad area contains several valid Lets, create a Folder to organize them.
The Folder supplies navigation; it is not a substitute for a single Let scope.

Do not split only because a Let is long or has many files. A large dataset,
book, or collection may remain one Let when it has one purpose and one
lifecycle. Conversely, a short note can require a split if it contains several
independent responsibilities.

## Agent Decision Rules

### Update an Existing Let

Update when incoming material fits the existing Let's scope without changing
that scope. It may add evidence, correct claims, refresh data, expand a
section, or add derived content, but it must not introduce a new independent
responsibility.

```text
incoming knowledge ⊆ existing Let scope
  → update the Let
```

Contradictory evidence normally updates the same Let with provenance and a
resolved or unresolved comparison. Contradiction alone is not a reason to fork
a new Let.

### Create a New Let

Create a Let when no existing scope covers the incoming knowledge and the
candidate passes all three tests:

1. It has a clear direct title and one-sentence scope.
2. It is independently useful with enough context to be opened or retrieved.
3. It has an expected lifecycle independent from neighboring Lets.

If a candidate is only a fragment, do not create a micro-Let merely to make
ingestion appear complete. Attach it to a fitting Let or retain it in the
Artifact Root until a valid integration target exists.

### Split a Let

Split when the existing scope statement has become false or must be stretched
to cover multiple independent responsibilities. The agent must first identify
the proposed child scopes and verify each child is independently useful.

```text
one Let
  ├── scope A: independently useful and separately maintained
  └── scope B: independently useful and separately maintained
→ two Lets, optionally grouped by one Folder
```

Splitting should preserve provenance and avoid dropping derived content. It is
a maintenance action, not a response to normal file length.

### Merge Lets

Merge only when Lets have effectively the same semantic responsibility and
lifecycle, and their separation causes duplication, contradiction, or
incompleteness. The merged Let must still have one precise scope statement.

```text
scope(A) ≈ scope(B)
and lifecycle(A) ≈ lifecycle(B)
and union(A, B) still has one scope
  → merge
```

Do not merge merely because Lets share a topic, Source, tag, Folder, or search
result. If their union would require a broader umbrella scope, keep them
separate and use a Folder or link.

### Link Lets

Link when Lets are related but remain independently useful and separately
maintained. Linking is the conservative alternative to an uncertain merge.

Examples of link relationships may include supporting evidence, comparison,
part-of, related topic, or continuation. The exact graph schema is deferred;
the boundary rule is not.

## Decision Procedure

For each candidate knowledge responsibility discovered from artifacts, the
agent follows this order:

1. State the candidate scope in one sentence.
2. Search existing Lets by stored scope, title, provenance, and relevant
   content; embeddings alone are insufficient evidence.
3. If one existing scope fully contains the candidate, update it.
4. If the candidate is related to existing Lets but has a distinct scope,
   create it if it passes the lower-bound tests, then link it.
5. If no existing scope contains it, create it only if it is independently
   useful; otherwise retain it as artifact material or add it to a fitting
   parent Let.
6. After a material update, validate that the target Let still has one scope.
   Split only if the upper-bound tests are met.
7. Merge only during high-confidence maintenance or when equivalent scopes are
   demonstrably causing harm.

The default is stability: prefer update or link over speculative split/merge.
An agent must not churn Let identities because a later prompt interprets the
same material slightly differently.

## Retrieval and Context Guardrails

“Just enough information” is semantic, not physical. However, each Let SHOULD
have an entrypoint such as `overview.md` or a manifest that explains its scope,
structure, and key contents within one configured retrieval budget. Large
attachments may remain in the Let and be loaded selectively.

Exceeding an entrypoint context budget first calls for a better index, table of
contents, or selective loading. It does not automatically require a split. A
split is required only when semantic scope or lifecycle also diverges.

## Boundary Anti-Patterns

| Anti-pattern | Why it fails | Correct action |
|---|---|---|
| One Let per upload | Treats transport boundary as knowledge boundary | Update, create, or link by scope |
| One Let per chunk | Creates unreadable micro-objects | Keep chunks as files inside a Let or artifacts |
| One giant topic Let | Hides independent retrieval and lifecycle boundaries | Split into Lets under a Folder |
| Merge by shared tag/source | Conflates relatedness with identity | Link related Lets |
| Split by byte size | Makes structure depend on storage rather than meaning | Index large Lets; split only on semantic divergence |
| Rewrite scope silently | Makes later agent behavior unstable | Treat material scope change as a boundary decision |

## Acceptance Criteria

- [ ] Every Let stores a stable title and scope statement.
- [ ] A normal update does not broaden the stored scope.
- [ ] The agent can explain whether an action was update, create, split, merge,
      or link using the three boundary dimensions.
- [ ] A fragment without independent usefulness does not become a user-visible
      micro-Let by default.
- [ ] A large Let is not split solely because of bytes, tokens, or file count.
- [ ] Related Lets can remain separate and linked.
- [ ] Split and merge preserve source provenance.

## References

- [Let Model](./let-model.md)
- [Brain Integration](./brain-integration.md)
- [Domain Model](./domain-model.md)
- [User Experience](./user-experience.md)
