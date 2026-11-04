# Dead-Turn Prune Contract

## Behavior

CURRENT: a pruned component is absent from staged and sealed current View, JSON,
XML, selector, and provider output.

HISTORY: the component remains addressable in every revision in which it was
live. Prune appends a deletion to history; it does not rewrite earlier
revisions.

REWIND: a consumer can resolve a dead handle by historical scan and fork from the
revision in which that component was live.

## Verification

1. Rust proves staged current absence, sealed current absence, and historical
   presence for a file-bound tree.
2. pactree-py proves the same observations through the public binding.
3. Consumer rewind tests use isolated storage and resolve the intended
   historical component before forking. Reusing persistent fixtures can leave
   duplicate component identities and invalidate the test's premise.
