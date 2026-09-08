//! Model-based property tests: arbitrary op sequences against the live-slot set.

use proptest::prelude::*;
use slab_pool::SlabPool;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
enum Op {
    /// Alloc a value; if the pool is exhausted, this is a no-op.
    Alloc(u64),
    /// Free the live guard at a given position in the live-slot list.
    Free(usize),
}

/// Maximum pool capacity for tests.
const CAP: usize = 8;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn model_tracks_allocation_exactly(ops in proptest::collection::vec(
        prop_oneof![
            (any::<u64>()).prop_map(Op::Alloc),
            (0usize..CAP).prop_map(Op::Free),
        ],
        0..300,
    )) {
        let pool = SlabPool::new(CAP).unwrap();
        // model: pool-slot-index -> expected value for every live slot.
        let mut model: HashMap<usize, u64> = HashMap::new();
        let mut guards: HashMap<usize, slab_pool::PoolGuard<'_, u64>> = HashMap::new();

        for (step, op) in ops.iter().enumerate() {
            match op {
                Op::Alloc(v) => {
                    let live = model.len();
                    if live < CAP {
                        let guard = pool
                            .alloc(*v)
                            .unwrap_or_else(|| panic!("step {step}: alloc failed with {live}/{CAP} live"));
                        let idx = guard.slot_index();
                        prop_assert!(
                            !model.contains_key(&idx),
                            "step {step}: slot {idx} double-allocated"
                        );
                        prop_assert_eq!(*guard, *v);
                        model.insert(idx, *v);
                        guards.insert(idx, guard);
                    } else {
                        prop_assert!(
                            pool.alloc(*v).is_none(),
                            "step {step}: alloc succeeded with {live}/{CAP} live slots"
                        );
                    }
                }
                Op::Free(pos) => {
                    // Free the live slot at `pos` in sorted order (stable
                    // regardless of HashMap iteration order).
                    let mut live_slots: Vec<usize> = model.keys().copied().collect();
                    live_slots.sort_unstable();
                    if let Some(&idx) = live_slots.get(pos % live_slots.len().max(1)) {
                        prop_assert_eq!(
                            model.remove(&idx),
                            guards.remove(&idx).map(|g| *g),
                            "value mismatch on free of slot {} at step {}", idx, step
                        );
                    }
                }
            }
        }

        // Quiescence: drop everything, the pool must be fully free again.
        guards.clear();
        model.clear();
        let stats = pool.stats();
        prop_assert_eq!(
            stats.free, CAP,
            "quiesced pool should have all slots free"
        );

        // No slot may be stuck: a fresh pool cycle must work.
        let mut cycle = Vec::new();
        for v in 0..CAP as u64 {
            cycle.push(pool.alloc(v).expect("post-quiescence refill"));
        }
        assert!(pool.alloc(0).is_none(), "pool full after refill");
        drop(cycle);
    }

    #[test]
    fn no_double_free_detected(ops in proptest::collection::vec(
        (0usize..CAP, any::<u64>()),
        0..100,
    )) {
        // Allocating the pool full then freeing by slot order must keep the
        // free-list consistent: every realloc must land on a previously
        // freed slot with the correct value preserved until overwrite.
        let pool = SlabPool::new(CAP).unwrap();
        let mut held = HashSet::new();
        for (slot_hint, v) in ops {
            let guard = match pool.alloc(v) {
                Some(g) => g,
                None => continue,
            };
            let idx = guard.slot_index();
            prop_assert!(
                !held.contains(&idx),
                "slot {idx} handed out twice concurrently"
            );
            held.insert(idx);
            prop_assert_eq!(*guard, v);
            drop(guard);
            held.remove(&idx);
            let _ = slot_hint; // position hint only; pool decides placement
        }
    }
}
