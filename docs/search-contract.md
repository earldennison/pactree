# PACT Search Contract

Status: frozen for Pactree v0.1

PACT search is a derived acceleration surface over canonical PACTBIN2 data.
PACT transactions, component identity, playback, and exact content remain the
only durable truth. Removing every semantic facet and catalog must not change
normal PACT behavior.

## Supported Surfaces

V0.1 search accepts only:

```text
head
branch:<branch-id>
```

`head` searches the current head of the selected PACT branch.
`branch:<branch-id>` searches the head of one explicit directly indexed branch.
Branch IDs must be non-empty.

The following surfaces are intentionally unsupported in v0.1:

```text
historical revisions
ledger-wide history
composed delta overlays
implicit persisted-overlay composition
```

Unsupported requests return a typed `PactErrorKind::Invalid` error. They never
fall back to the current head and never return a partial local-only result.
Persisted overlays may be searched as their raw local branch. An ephemeral host
overlay may later contribute an in-memory mini-facet, but it is never persisted
implicitly.

## Search Modes

The public modes are:

```text
exact
regex
lexical
dense
hybrid
```

`exact` and `regex` use derived data only to generate candidates. Final matches
are verified against canonical bytes from the pinned PACT snapshot. `lexical`
uses versioned prose and symbol analyzers. `dense` scores inside one explicit
embedding space. `hybrid` uses a versioned deterministic fusion policy.

Selectors retain their existing PACT syntax and ordering semantics. A selector
determines eligibility before ranking. Catalog filters are separate structured
fields and never reinterpret selector text.

## Snapshot Identity

Every search pins one `PactSearchSnapshotInfo`:

```text
source_file_identity
root_id
branch_id
head_revision
search_cursor
physical_generation
```

`source_file_identity` identifies the physical PACT source. `root_id` identifies
the logical context root. They are separate identities and must not alias.

The search cursor is qualified by source file identity, root identity, branch,
semantic revision, and ordinal. A cursor from another identity or branch is
rejected.

`physical_generation` selects and diagnoses the pinned storage snapshot. It is
not a semantic invalidation key: physical compaction alone does not make a
logically complete facet stale.

## Freshness

The default policy is `require_current`. A query fails when the facet does not
cover every search-affecting change visible in the pinned PACT snapshot.

`allow_stale` permits an explicitly stale facet only when the result reports its
freshness and coverage. Staleness is never hidden.

A facet is current only when all of these match:

```text
source file identity
root identity
branch
search-change cursor coverage
retrieval-unit contract
chunker contract
embedding-space contract
lexical-analyzer contract
index policy
```

Physical generation equality is not required.

## Boundedness And Consistency

Snapshot identity, selector eligibility, source-field reads, scoring locators,
and final hit hydration must use the same pinned PACT read handle. A concurrent
append cannot mix a newer source value into an older scored result.

Search query, rebuild, incremental sync, selector filtering, and hit hydration
must not materialize a complete ledger. Limits are enforced in bytes and tokens
as well as rows. Large tool parameters, results, errors, media, and externalized
content are read in bounded ranges.

The pinned `PactFileReadHandle` supplies `search_snapshot_info`,
`search_selector_eligibility`, `search_source_page`, `read_search_field`,
`stream_content_range`, and `hydrate_search_hits`. `search_source_page` pages
payload-free component metadata under independent row and serialized-byte
ceilings, using the same ordinals as selector eligibility. It rejects composed
branches instead of exposing an incomplete local-only surface. Selector
eligibility preserves the public PACT selector grammar but is not subject to
the public 1,000-result presentation cap. It returns stable branch-snapshot
ordinals: zero is the root and remaining values are the shallow component-index
order plus one. Exact indexes produce sparse bitmaps; broad node-local filters
use dense metadata scans; structural filters use the parent-child facet and
shallow component metadata. None hydrates the ledger or externalized payloads.
Historical/composed eligibility is explicitly unsupported unless the caller
requests the expensive fallback plan; it is never silently treated as
current-head eligibility.
Text continuations are byte-addressed but always end on UTF-8 boundaries;
resuming from a non-boundary is an invalid request. Final-hit hydration shares
the handle's immutable mapping, so an append cannot mix a newer field into an
older scored result. Only explicit `refresh()` advances the handle.

Canonical PACT append never waits for embedding, catalog updates, or semantic
facet maintenance. Derived search work may lag and report that lag through
freshness and coverage.

## Search-Affecting Change Feed

`file_search_changes(path, branch, cursor, max_events, max_metadata_bytes)` is
the only incremental semantic-index input. It reads the PACTBIN2 B+tree
directly and never hydrates the ledger. Pages are exclusive after the supplied
cursor and bounded by both event count and serialized event metadata.

Every cursor is qualified by `source_file_identity`, `root_id`, and
`branch_id`. A mismatch is a typed invalid request. Physical generation is not
part of cursor validity, so in-place compaction that preserves source identity
also preserves incremental search progress.

The feed distinguishes field upserts/deletes, context invalidation,
visibility/lifecycle changes, and relationship changes. Dependency scope is
explicitly one component, its parent turn, or branch context. Events contain
references and hashes, never large content payloads. Role, tool name, success,
tool pairing, parent movement, lifecycle, visibility, deletion, and message
ordering therefore invalidate semantic retrieval even when text is unchanged.

`file_text_changes` remains a separate compatibility API with unchanged JSON
and cursor semantics. Semantic indexing must not infer completeness from that
text-only feed.

## Failure Rules

Search fails explicitly when:

- the requested surface is unsupported;
- the source, root, or branch identity does not match the cursor or facet;
- `require_current` is selected and the facet is stale or incomplete;
- selector eligibility cannot be evaluated under the requested bounded policy;
- an embedding-space or analyzer contract is incompatible;
- final canonical hydration cannot verify a result locator.

Deleting or corrupting a semantic facet removes acceleration only. Playback,
content recovery, selectors, fork, rewind, slice, merge, heal, and ordinary PACT
reads remain available from canonical PACTBIN2 data.
