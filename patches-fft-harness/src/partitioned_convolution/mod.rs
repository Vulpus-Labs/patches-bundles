//! Uniform and non-uniform partitioned convolution using overlap-save.
//!
//! The impulse response (IR) is split into fixed-size partitions, each
//! pre-transformed via a 2N-point real FFT. Input arrives in N-sample blocks;
//! a frequency-domain delay line (FDL) accumulates the contribution of each
//! partition for correct linear convolution without circular artifacts.
//!
//! All buffers are pre-allocated at construction time — `process_block`
//! performs zero heap allocations.

mod complex;
mod convolver;
mod ir_partitions;
mod non_uniform;

pub use complex::{complex_multiply_accumulate_packed, complex_multiply_packed};
pub use convolver::PartitionedConvolver;
pub use ir_partitions::IrPartitions;
pub use non_uniform::NonUniformConvolver;

#[cfg(test)]
mod tests;
