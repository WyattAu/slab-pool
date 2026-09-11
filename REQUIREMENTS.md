# Requirements — slab-pool

Numbered, testable requirements. Every requirement maps to at least one named
test; every security-relevant test cites at least one requirement. Threat
IDs reference `THREAT-MODEL.md`.

Scope note: `slab-pool` provides a lock-free slab object pool — tagged-pointer
ABA protection on the free list, RAII `PoolGuard` return-to-pool, fixed
capacity with non-blocking exhaustion, and `PoolStats` diagnostics.

## Functional

| ID | Requirement | Priority |
|----|-------------|----------|
| REQ-SP-001 | `SlabPool::new(capacity)` builds a pool of exactly `capacity` slots; zero or oversized (`> MAX_SLOTS`) capacities are rejected with `SlabPoolError` | MUST |
| REQ-SP-002 | `alloc` returns a `PoolGuard` granting exclusive access to a slot while live; exhausted pools return `None` without blocking | MUST |
| REQ-SP-003 | Dropping a `PoolGuard` returns the slot to the pool and runs the value's `Drop` exactly once | MUST |
| REQ-SP-004 | Freed slots are reusable and recycled LIFO-first; a value written into a slot never leaks into a later allocation's observable state | MUST |
| REQ-SP-005 | `capacity` and `stats` report capacity, live count, and free count consistently under concurrent use | SHOULD |
| REQ-SP-006 | `slot_index` maps a live guard to its slot number deterministically | SHOULD |

## Security

| ID | Requirement | Priority |
|----|-------------|----------|
| REQ-SP-100 | ABA reuse cannot corrupt the free list: the tagged pointer increments a generation tag on every pop/push, so a stale CAS fails instead of resurrecting an old state (T1) | MUST |
| REQ-SP-101 | Concurrent `alloc` cannot hand out the same slot twice: the head CAS transfers ownership atomically (T2) | MUST |
| REQ-SP-102 | Double-free / use-after-drop is impossible through the safe API: slot return happens only in `PoolGuard::drop` on a guard that no longer dereferences after return (T3) | MUST |
| REQ-SP-103 | Invalid construction (zero or `> MAX_SLOTS` capacity) is rejected eagerly — no lazy failure surface (T5) | MUST |
| REQ-SP-104 | The lock-free protocol is model-checked under loom: no double-alloc and stable push/pop roundtrips across all bounded interleavings | MUST |

## Robustness

| ID | Requirement | Priority |
|----|-------------|----------|
| REQ-SP-200 | Exhaustion is a non-blocking `None` (T4): callers never spin, lock, or wait on pool state | MUST |
| REQ-SP-201 | Concurrent stress (many threads alloc/free) never exceeds live capacity and never loses slots: live + free == capacity invariant holds | MUST |

## Traceability Matrix

| Requirement | Test (fn, file) | Property class |
|-------------|-----------------|----------------|
| REQ-SP-001 | `zero_capacity_is_error` (`src/lib.rs` tests), `new` | unit |
| REQ-SP-002 | `exhaustion_returns_none`, `basic_alloc_deref_drop` | unit |
| REQ-SP-003 | `basic_alloc_deref_drop`, `aba_freed_slot_is_reusable` | unit |
| REQ-SP-004 | `aba_freed_slot_is_reusable`, `lifo_reuse_order`, `value_integrity_across_realloc` | unit |
| REQ-SP-005 | `live_never_exceeds_capacity`, `stats` assertions in stress test | unit/stress |
| REQ-SP-100 | `aba_freed_slot_is_reusable`, `loom_concurrent_alloc_free_no_double_alloc` | unit/loom |
| REQ-SP-101 | `concurrent_stress_no_over_allocation`, `loom_concurrent_alloc_free_no_double_alloc` (`tests/*.rs`) | stress/loom |
| REQ-SP-102 | `basic_alloc_deref_drop`, `value_integrity_across_realloc` | unit |
| REQ-SP-103 | `zero_capacity_is_error` | unit |
| REQ-SP-104 | `loom_concurrent_alloc_free_no_double_alloc`, `loom_push_pop_roundtrip_stable` (`tests/loom.rs`) | loom |
| REQ-SP-200 | `exhaustion_returns_none` | unit |
| REQ-SP-201 | `concurrent_stress_no_over_allocation`, `live_never_exceeds_capacity` | stress |

## Test Count

- 10 `#[test]` functions across unit, stress, and loom suites.
- All-features suite passes with 0 failures; no-default-features suite passes.
