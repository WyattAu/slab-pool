//! Criterion benchmarks for the slab-pool alloc/free hot path.

use criterion::{criterion_group, criterion_main, Criterion};
use slab_pool::SlabPool;
use std::sync::Arc;

fn bench_alloc_drop_uncontended(c: &mut Criterion) {
    let pool = SlabPool::new(64).unwrap();
    c.bench_function("alloc_drop_uncontended", |b| {
        b.iter(|| {
            let guard = pool.alloc(42u64).expect("capacity");
            criterion::black_box(&*guard);
            drop(guard);
        })
    });
}

fn bench_alloc_drop_contended(c: &mut Criterion) {
    let pool = Arc::new(SlabPool::new(64).unwrap());
    let mut group = c.benchmark_group("alloc_drop_contended");
    for threads in [2usize, 4, 8] {
        group.throughput(criterion::Throughput::Elements(10_000u64 * threads as u64));
        group.bench_function(format!("{threads}_threads"), |b| {
            b.iter(|| {
                let mut handles = Vec::new();
                for _ in 0..threads {
                    let pool = Arc::clone(&pool);
                    handles.push(std::thread::spawn(move || {
                        for _ in 0..10_000u64 {
                            let guard = pool.alloc(0u64).expect("capacity");
                            criterion::black_box(&*guard);
                            drop(guard);
                        }
                    }));
                }
                for h in handles {
                    h.join().expect("no panic");
                }
            })
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_alloc_drop_uncontended,
    bench_alloc_drop_contended
);
criterion_main!(benches);
