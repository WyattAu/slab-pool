#![deny(missing_docs)]
#![warn(clippy::undocumented_unsafe_blocks)]

//! Lock-free slab object pool with tagged-pointer ABA protection.
//!
//! A fixed-capacity object pool where `alloc` and free are O(1) lock-free
//! CAS operations on a Treiber-stack free list threaded through the slots.
//! The free-list head packs `(index: u32, tag: u32)` into an `AtomicU64`;
//! the tag increments on every free-list push, making ABA reuse impossible
//! without 2³² re-pushes of the same slot between a reader's load and CAS.
//!
//! # Why not a mutex?
//!
//! A mutex-sharded pool is correct and sufficient for most workloads. This
//! crate exists for the case where even brief lock hold times are
//! unacceptable: a CAS loop on an uncontended head is ~5 ns vs ~20–40 ns
//! for an uncontended mutex lock/unlock pair, and under contention the
//! Treiber stack degrades to spin-retry rather than futex sleep.
//!
//! # ABA protection
//!
//! The classic Treiber-stack ABA hazard: thread A reads head = (slot 3,
//! tag 5), gets preempted. Meanwhile slot 3 is freed, re-allocated, and
//! freed again — the head is back at (3, ?) but the free-list *next*
//! pointer may have changed. Thread A's CAS on (3, tag 5) would succeed
//! against an untagged head if only the index matched — silently corrupting
//! the list. With a tagged head, slot 3's second free bumps the tag to 7,
//! so thread A's CAS on tag 5 fails and it retries with fresh state.
//!
//! # Guard-based exclusivity
//!
//! [`alloc`](SlabPool::alloc) returns a [`PoolGuard`] that owns the slot
//! until dropped. Two guards to the same slot are impossible: the slot is
//! removed from the free list on alloc and returned on drop. Multiple
//! guards to *different* slots are safe and expected.
//!
//! # Platform
//!
//! Works on any platform with `AtomicU64` (all 64-bit, most 32-bit). No
//! `cmpxchg16b` needed: the u64 packing avoids the 128-bit CAS portability
//! trap entirely.

mod error;

pub use error::SlabPoolError;

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

/// Index of the "empty" free-list terminator. Slot indices are 1-based.
const NIL: u64 = 0;

/// Number of bits for the slot index in the packed head.
const IDX_BITS: u64 = 32;

/// Extracts the slot index from a packed head word.
#[inline(always)]
fn unpack_idx(packed: u64) -> u64 {
    packed >> IDX_BITS
}

/// Extracts the ABA tag from a packed head word.
#[inline(always)]
fn unpack_tag(packed: u64) -> u64 {
    packed & ((1 << IDX_BITS) - 1)
}

/// Packs a slot index and ABA tag into a single head word.
#[inline(always)]
fn pack(idx: u64, tag: u64) -> u64 {
    (idx << IDX_BITS) | (tag & ((1 << IDX_BITS) - 1))
}

/// A fixed-capacity, lock-free slab object pool.
///
/// # Example
///
/// ```
/// use slab_pool::SlabPool;
///
/// let pool = SlabPool::new(4).unwrap();
/// let mut guard = pool.alloc(42).expect("pool has capacity");
/// assert_eq!(*guard, 42);
/// *guard = 99;
/// assert_eq!(*guard, 99);
/// drop(guard); // slot returns to the free list
/// let guard2 = pool.alloc(7).expect("slot was returned");
/// assert_eq!(*guard2, 7);
/// ```
pub struct SlabPool<T> {
    /// Pre-allocated slot storage.
    slots: Box<[Slot<T>]>,
    /// Packed (index << 32 | tag) Treiber-stack free-list head.
    head: AtomicU64,
    /// Total capacity (for stats).
    capacity: u32,
    /// Marker for variance.
    _marker: PhantomData<T>,
}

/// Internal slot representation.
struct Slot<T> {
    /// The stored value (valid only while allocated).
    value: UnsafeCell<MaybeUninitSlot<T>>,
    /// Next free slot index (valid only while on the free list).
    next_free: AtomicU64,
}

/// Alias to keep the MaybeUninit plumbing readable.
type MaybeUninitSlot<T> = std::mem::MaybeUninit<T>;

// SAFETY: `T: Send` is sufficient. The free-list CAS ensures exclusive slot
// ownership via `PoolGuard`; no two threads can hold a guard to the same
// slot simultaneously, so `T: Send` (value moves between threads on
// alloc/drop) is all that's needed. No `Sync` bound on `T` because shared
// references to slot contents are never created.
unsafe impl<T: Send> Send for SlabPool<T> {}
// SAFETY: same exclusivity argument as `Send` — all cross-thread access to
// slot contents goes through exclusively-owned `PoolGuard`s; the free list
// itself is manipulated only via atomic CAS.
unsafe impl<T: Send> Sync for SlabPool<T> {}

impl<T> SlabPool<T> {
    /// Creates a new pool with the given capacity.
    ///
    /// All slots are on the free list initially. Does not require `T:
    /// Default` — slots are populated on `alloc`.
    ///
    /// # Errors
    ///
    /// Returns [`SlabPoolError::ZeroCapacity`] if `capacity == 0`, or
    /// [`SlabPoolError::CapacityOverflow`] if capacity exceeds the
    /// addressable slot count.
    pub fn new(capacity: usize) -> Result<Self, SlabPoolError> {
        if capacity == 0 {
            return Err(SlabPoolError::ZeroCapacity);
        }
        // Slot indices are 1-based u32 values packed into the head word, so
        // capacity must fit in u32 (slot 0 is the NIL sentinel).
        if capacity > u32::MAX as usize {
            return Err(SlabPoolError::CapacityOverflow(capacity));
        }

        // Thread the free list: slot 1 → slot 2 → ... → slot capacity → NIL.
        // Head starts at (1, tag 0).
        let mut slots: Vec<Slot<T>> = Vec::with_capacity(capacity);
        for i in 1..=capacity {
            let next = if i < capacity { (i + 1) as u64 } else { NIL };
            // SAFETY: the free-list next pointer is only meaningful while
            // the slot is unallocated; `MaybeUninit` value is uninitialised
            // until `alloc` writes it.
            slots.push(Slot {
                value: UnsafeCell::new(MaybeUninitSlot::uninit()),
                next_free: AtomicU64::new(next),
            });
        }

        Ok(Self {
            slots: slots.into_boxed_slice(),
            head: AtomicU64::new(pack(1, 0)),
            capacity: capacity as u32,
            _marker: PhantomData,
        })
    }

    /// Allocates a slot, moving `value` into it.
    ///
    /// Returns `None` if the pool is exhausted (all slots are held by live
    /// guards). Lock-free: a CAS loop on the packed free-list head.
    ///
    /// # ABA safety
    ///
    /// The tagged head makes stale `(index, tag)` observations fail the CAS.
    /// See the crate-level docs for the full ABA scenario.
    pub fn alloc(&self, value: T) -> Option<PoolGuard<'_, T>> {
        let mut head = self.head.load(Ordering::Acquire);
        loop {
            let idx = unpack_idx(head);
            if idx == NIL {
                return None; // Pool exhausted.
            }
            let tag = unpack_tag(head);

            // SAFETY: `idx` is in `1..=capacity` (validated at construction,
            // maintained by the free-list discipline); the index is within
            // `slots` bounds. Reading `next_free` here is racy — another
            // thread may have pushed a different slot — but the subsequent
            // CAS validates that the head is still `(idx, tag)`, which means
            // the free-list layout was correct at the moment of the read.
            // This is the standard Treiber-stack safety argument.
            //
            // Note: `next_free` is read through a plain shared reference to
            // the `AtomicU64` field (sound: atomics are `Sync`, and the value
            // itself is only meaningful after a successful CAS).
            let next = self.slots[idx as usize - 1]
                .next_free
                .load(Ordering::Acquire);

            let new_head = pack(next, tag.wrapping_add(1));
            match self.head.compare_exchange_weak(
                head,
                new_head,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    let cell = &self.slots[idx as usize - 1].value;
                    // SAFETY: exclusive access — the slot is off the free
                    // list (CAS succeeded), and no other thread can reach it
                    // without going through the free list.
                    unsafe { (*cell.get()).write(value) };
                    return Some(PoolGuard {
                        pool: self,
                        idx: idx as usize,
                        _marker: PhantomData,
                    });
                }
                Err(observed) => {
                    head = observed; // Retry with the fresh head.
                }
            }
        }
    }

    /// Returns the total capacity of the pool.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity as usize
    }

    /// Returns approximate pool statistics (Relaxed loads — approximate
    /// under contention, exact at quiescence).
    ///
    /// `allocated = capacity - free`. The `free` count is derived from the
    /// free-list walk and may briefly mis-count under concurrent alloc/free,
    /// but converges when the pool is quiescent.
    #[must_use]
    pub fn stats(&self) -> PoolStats {
        let head = self.head.load(Ordering::Acquire);
        let head_idx = unpack_idx(head);
        let mut free = 0usize;
        let mut cursor = head_idx;
        while cursor != NIL {
            free += 1;
            // SAFETY: free-list traversal — cursor is in `1..=capacity` by
            // construction (free-list discipline maintains valid indices).
            cursor = self.slots[cursor as usize - 1]
                .next_free
                .load(Ordering::Acquire);
        }
        PoolStats {
            capacity: self.capacity as usize,
            free,
        }
    }

    /// Returns a slot to the free list (CAS push with tag increment).
    ///
    /// # Safety
    ///
    /// The caller must guarantee that `idx` is currently allocated (i.e.,
    /// removed from the free list by a successful `alloc` CAS) and that no
    /// other thread will return the same index. The `PoolGuard` Drop impl
    /// is the only intended caller.
    /// Returns a slot to the free list (lock-free Treiber push, retries on
    /// CAS failure).
    ///
    /// # Safety
    ///
    /// The caller must guarantee that `idx` is currently allocated (removed
    /// from the free list by a successful `alloc` CAS) and that no other
    /// thread will return the same index. `PoolGuard`'s Drop impl is the
    /// only intended caller; the RAII guard guarantees exclusivity.
    unsafe fn return_slot(&self, idx: usize) {
        loop {
            let head = self.head.load(Ordering::Acquire);
            let tag = unpack_tag(head);
            let old_head_idx = unpack_idx(head);

            // Point our slot's next at the current head.
            //
            // SAFETY: idx is within bounds per the alloc contract.
            self.slots[idx - 1]
                .next_free
                .store(old_head_idx, Ordering::Release);

            // Publish: bump tag to invalidate stale ABA observations.
            let new_head = pack(idx as u64, tag.wrapping_add(1));
            if self
                .head
                .compare_exchange(head, new_head, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return;
            }
            // Head changed — another thread pushed or allocated. Retry with
            // the fresh head (standard Treiber-stack push retry).
        }
    }
}

impl<T> fmt::Debug for SlabPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlabPool")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

/// RAII guard for a pool slot. The slot returns to the free list on drop.
pub struct PoolGuard<'pool, T> {
    pool: &'pool SlabPool<T>,
    idx: usize,
    _marker: PhantomData<*mut T>,
}

// SAFETY: `T: Send` — the guard owns exclusive access to the slot's value,
// and dropping it on another thread is a move (Send for the guard means the
// value can be dropped from another thread, which is fine for `T: Send`).
unsafe impl<'pool, T: Send> Send for PoolGuard<'pool, T> {}

impl<'pool, T> PoolGuard<'pool, T> {
    /// Returns the slot index (1-based within the pool).
    #[must_use]
    pub fn slot_index(&self) -> usize {
        self.idx
    }
}

impl<'pool, T> std::ops::Deref for PoolGuard<'pool, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the slot was written by `alloc` and is exclusively owned
        // by this guard until drop.
        unsafe { (*self.pool.slots[self.idx - 1].value.get()).assume_init_ref() }
    }
}

impl<'pool, T> std::ops::DerefMut for PoolGuard<'pool, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: exclusive mutable access, same exclusivity argument.
        unsafe { (*self.pool.slots[self.idx - 1].value.get()).assume_init_mut() }
    }
}

impl<'pool, T> Drop for PoolGuard<'pool, T> {
    fn drop(&mut self) {
        // SAFETY: this guard holds the only reference to the slot (removed
        // from the free list at alloc time). Dropping the value here is
        // correct — the caller may have already replaced it via DerefMut.
        //
        // `ptr::drop_in_place` on `assume_init_mut().as_mut_ptr()` is the
        // stable-Rust equivalent of the nightly `assume_init_drop`.
        unsafe {
            let ptr = (*self.pool.slots[self.idx - 1].value.get()).as_mut_ptr();
            std::ptr::drop_in_place(ptr);
        }
        // SAFETY: this guard owned the slot; returning it is the drop
        // contract.
        unsafe { self.pool.return_slot(self.idx) };
    }
}

/// Point-in-time pool statistics (approximate under contention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    /// Total capacity.
    pub capacity: usize,
    /// Approximate number of slots on the free list.
    pub free: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_alloc_deref_drop() {
        let pool = SlabPool::new(4).unwrap();
        let mut g = pool.alloc(42).unwrap();
        assert_eq!(*g, 42);
        *g = 99;
        assert_eq!(*g, 99);
        drop(g);
        let g2 = pool.alloc(7).unwrap();
        assert_eq!(*g2, 7);
    }

    #[test]
    fn exhaustion_returns_none() {
        let pool = SlabPool::new(2).unwrap();
        let a = pool.alloc(1).unwrap();
        let b = pool.alloc(2).unwrap();
        assert!(pool.alloc(3).is_none(), "pool exhausted");
        drop(a);
        let c = pool.alloc(3).unwrap();
        assert_eq!(*c, 3);
        drop(b);
        drop(c);
    }

    #[test]
    fn zero_capacity_is_error() {
        assert_eq!(
            SlabPool::<u8>::new(0).unwrap_err(),
            SlabPoolError::ZeroCapacity
        );
    }

    #[test]
    fn lifo_reuse_order() {
        let pool = SlabPool::new(4).unwrap();
        let a = pool.alloc(1).unwrap();
        let a_idx = a.slot_index();
        let b = pool.alloc(2).unwrap();
        let b_idx = b.slot_index();
        let c = pool.alloc(3).unwrap();
        let c_idx = c.slot_index();
        drop(c);
        drop(b);
        // LIFO: last freed (b) is first re-allocated.
        let next = pool.alloc(4).unwrap();
        assert_eq!(next.slot_index(), b_idx);
        // Slot `b` is now held by `next`; the next free slot is c's old one.
        let next2 = pool.alloc(5).unwrap();
        assert_eq!(next2.slot_index(), c_idx);
        drop(next);
        drop(next2);
        drop(a);
        // Sanity: a's slot (freed last) comes back first.
        let next3 = pool.alloc(6).unwrap();
        assert_eq!(next3.slot_index(), a_idx);
        drop(next3);
    }

    #[test]
    fn value_integrity_across_realloc() {
        let pool = SlabPool::new(2).unwrap();
        {
            let mut g = pool.alloc(String::from("hello")).unwrap();
            g.push_str(" world");
            assert_eq!(&**g, "hello world");
        } // Dropped — String value is properly dropped.
        let g = pool.alloc(String::from("replacement")).unwrap();
        assert_eq!(&**g, "replacement");
    }

    #[test]
    fn concurrent_stress_no_over_allocation() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let pool = Arc::new(SlabPool::new(4).unwrap());
        let live = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        for _ in 0..8 {
            let pool = Arc::clone(&pool);
            let live = Arc::clone(&live);
            handles.push(std::thread::spawn(move || {
                for _ in 0..10_000 {
                    // Only count guards that were actually allocated — a
                    // `None` from an exhausted pool must not inflate the count.
                    if let Some(guard) = pool.alloc(0u64) {
                        let count = live.fetch_add(1, Ordering::AcqRel) + 1;
                        assert!(count <= 4, "over-allocation detected: {count} live");
                        live.fetch_sub(1, Ordering::AcqRel);
                        drop(guard);
                    }
                }
            }));
        }
        for h in handles {
            h.join().expect("no thread panicked");
        }
    }
}
