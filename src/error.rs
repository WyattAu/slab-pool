//! Errors returned by [`SlabPool`](crate::SlabPool) construction.

/// Maximum addressable slots (u32 index space, slot 0 reserved as sentinel).
pub const MAX_SLOTS: usize = (u32::MAX - 1) as usize;

/// Errors from [`SlabPool`](crate::SlabPool) construction.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SlabPoolError {
    /// Capacity must be at least 1.
    #[error("capacity must be >= 1, got 0")]
    ZeroCapacity,

    /// Capacity exceeds addressable slots (u32 index space, slot 0 reserved).
    #[error("capacity {capacity} exceeds the maximum addressable slot count of {max}", capacity = .0, max = MAX_SLOTS)]
    CapacityOverflow(usize),
}
