# slab-pool

Lock-free slab object pool with tagged-pointer ABA protection and RAII guards. Fixed-capacity object reuse with O(1) lock-free alloc/free — a CAS loop on an uncontended head is ~5ns vs ~20–40ns for mutex lock/unlock, and degrades to spin-retry rather than futex sleep under contention.

## Quick start

```rust
use slab_pool::SlabPool;

let pool = SlabPool::new(4).unwrap();
let mut guard = pool.alloc(42).expect("capacity");
assert_eq!(*guard, 42);
*guard = 99;
assert_eq!(*guard, 99);
drop(guard); // slot returns to the free list
let again = pool.alloc(7).expect("slot was returned");
assert_eq!(*again, 7);
```

## Design

- **Treiber-stack free list** threaded through slots, O(1) alloc/free.
- **Tagged head**: `(index: u32, tag: u32)` packed in an `AtomicU64`. The tag increments on every free-list mutation, making stale `(index, tag)` observations fail the CAS — the classic ABA hazard resolved without hazard pointers or epochs.
- **Guard-based exclusivity**: `alloc` returns `PoolGuard`, which owns the slot until dropped. Two guards to the same slot are impossible; the `PoolGuard`'s `Drop` returns the slot. The widespread Treiber double-free footgun is a type error here, not a runtime hazard.
- **Portable**: u64 packing needs only `AtomicU64` (all 64-bit, most 32-bit) — no `cmpxchg16b`/`AtomicU128` portability trap.

## Verification

- 6 unit tests + 2 proptest suites (~1,000 model-checked op sequences against a live-slot-set oracle).
- `tests/loom.rs`: concurrent alloc/free models (no double-alloc, push/pop roundtrip).
- `tests/kani.rs`: bounded proofs (live ≤ capacity; freed slots reusable — the ABA property).
- `cargo +nightly miri test` on the unsafe pointer code.

## Comparison

| Crate | Free | Model | Use when |
|---|---|---|---|
| `typed-arena` | No (drop-only) | `&`-allocated arenas | lifetimes allow borrowing |
| `bumpalo` | No (bump/reset) | bump pointer | allocation-only phases |
| QuestHive `OrderArena` | Yes (intrusive) | domain-typed, untagged pointers | never — use this crate |
| **slab-pool** | Yes (RAII guards) | generic, tagged, lock-free | fixed-capacity reuse across threads |

## License

MIT OR Apache-2.0.
