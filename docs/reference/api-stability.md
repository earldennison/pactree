# API Stability

Pactree 0.1 protects the public Rust surface recorded in
`api/pactree-0.1.0.txt`, the CLI help snapshot, public JSON schemas under
`contracts/schemas`, and normalized consumer observations under `contracts`.

Patch releases preserve public module paths, signatures, defaults, return
types, error families, ordering, identity, lifecycle behavior, serialization,
and documented side effects. Additive APIs require tests and documentation.

The storage layout is private. Derived indexes, checkpoints, caches, and page
addresses may change when public results remain identical. Every durable
semantic change must still be represented by the canonical transaction trace;
derived structures must be rebuildable and parity-validated.

`pactree-py` is separately versioned. Its public classes, methods, exceptions,
close behavior, and refresh semantics are protected by its own checked API
baseline and the shared consumer suite.
