# PATTERN_ADOPTION: slab-pool

Relationship to the WyattAu pattern canon. Statuses follow the closed
vocabulary: adopted, partial, planned, refused, n/a. Evidence paths are
relative to this repo.

| Pattern | Status | Evidence |
|---|---|---|
| P52 | n/a | Fixed-capacity generic object pool (src/lib.rs:92-96, capacity set at src/lib.rs:136). The canon bulkhead pattern partitions resources per tenant to contain starvation; this crate has no tenant dimension, so the pattern never applies |
