# Threat Model — slab-pool

Reference: STRIDE. Scope: the crate's public API surface (`SlabPool::new`,
`alloc`, `PoolGuard`, `stats`) as used by a concurrent downstream service.
Trust boundaries: (1) concurrent threads sharing one pool handle, (2) the
`T` values stored by callers, (3) the `unsafe` free-list machinery itself.

This crate has a **narrow security-sensitive surface**: it is an in-process
memory allocator with no I/O, no parsing of untrusted bytes, and no
identity concept. The credible threats are memory-safety and
availability-shaped, not adversarial.

## Assets

| ID | Asset | Example |
|----|-------|---------|
| A1 | Memory safety of pooled slots | ABA race handing one slot to two guards |
| A2 | Capacity discipline (exactly N live guards max) | Over-allocation corrupting caller invariants |

## STRIDE Analysis

| # | Threat | Category | Surface | Mitigation | Verifying test |
|---|--------|----------|---------|------------|----------------|
| T1 | ABA reuse corrupting the free list | Tampering | `alloc`, `return_slot` | Tagged-pointer head: `(index: u32, tag: u32)` packed in an `AtomicU64`; every free bumps the tag, so a stale `(idx, tag)` observation fails its CAS and retries | `aba_freed_slot_is_reusable`, `no_double_free_detected` (`tests/proptest.rs`), `loom_concurrent_alloc_free_no_double_alloc` (`tests/loom.rs`), `kani.rs` proofs |
| T2 | Double alloc of the same slot under contention | Tampering | `alloc` CAS loop | Compare-exchange on the packed head grants exclusivity before the slot is written; exhaustivity of the model-check covers the interleavings | `concurrent_stress_no_over_allocation` (8 threads × 10k iters assert ≤ capacity live guards), `loom_concurrent_alloc_free_no_double_alloc` |
| T3 | Use-after-drop / double free of slot values | Tampering | `PoolGuard::drop` | RAII: drop runs `drop_in_place` exactly once and is the only caller of `return_slot`; the guard owns exclusive access from successful CAS to drop | `value_integrity_across_realloc` (String properly dropped and replaced), `basic_alloc_deref_drop` |
| T4 | Exhaustion stalling callers | DoS | `alloc` | Pool exhaustion is a *non-blocking* `None` return — no lock to hold, no unbounded spin (CAS loop retries only against concurrent mutation, bounded by contention); callers implement their own backpressure | `exhaustion_returns_none`; stress test proves liveness under 8-thread contention |
| T5 | Invalid construction (zero or oversized capacity) | DoS | `SlabPool::new` | Eager validation returns `SlabPoolError::ZeroCapacity` / `CapacityOverflow` instead of panicking or mis-indexing later | `zero_capacity_is_error`; `CapacityOverflow` branch (`src/lib.rs:142`) |
| T6 | Stale slot bytes leaking prior values | Info disclosure | `alloc` reuse | Not exploitable through the safe API: a slot is exclusive to one guard between write and drop; `Debug` for `SlabPool` renders capacity only, never contents (`finish_non_exhaustive`) | `Debug` impl (`src/lib.rs:304`); exclusivity argument in `unsafe impl Sync` SAFETY comment |

## Repudiation

Not applicable — an allocator keeps no history and has no actors.

## Out of Scope

- What callers store in slots: secrets placed in a pool survive in freed
  slot bytes until overwritten; there is no zeroize-on-drop. That is `T`'s
  concern (use an explicit-wipe type if needed).
- The `stats()` walk is approximate under contention by design (documented);
  it is not a synchronization primitive.
- The `unsafe` blocks themselves are review-gated (documented SAFETY
  comments, `warn(clippy::undocumented_unsafe_blocks)`) rather than
  provable from safe code alone; loom/Kani/proptest cover the *logic*.

## Residual Risks

- **R1 (Low, accepted):** Tag wraparound: defeating the ABA tag requires
  2³² re-pushes of the same slot between one thread's load and CAS —
  computationally implausible and the crate documents the bound. Accepted
  as the standard tagged-pointer trade.
- **R2 (Low, accepted):** `MaybeUninit` slots hold prior bytes after drop;
  a future API bug that exposed an uninit slot would leak stale values.
  Guarded today solely by the guard-exclusivity invariant (T6) — no defense
  in depth (no poison pattern).
- **R3 (Low, accepted):** 32-bit slot index caps capacity at 2³²−1 slots;
  construction rejects overflow eagerly, so no runtime misindexing path.
