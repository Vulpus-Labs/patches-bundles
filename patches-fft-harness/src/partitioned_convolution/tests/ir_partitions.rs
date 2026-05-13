use super::*;

#[test]
fn ir_partitions_count() {
    let ir = vec![1.0; 100];
    let parts = IrPartitions::from_ir(&ir, 32);
    // ceil(100/32) = 4
    assert_eq!(parts.num_partitions(), 4);
    assert_eq!(parts.block_size(), 32);
    assert_eq!(parts.fft_size(), 64);
}

#[test]
fn ir_partitions_exact_fit() {
    let ir = vec![1.0; 64];
    let parts = IrPartitions::from_ir(&ir, 32);
    assert_eq!(parts.num_partitions(), 2);
}

#[test]
fn ir_partition_roundtrip() {
    // Verify that IFFT of each partition recovers the original IR segment.
    let ir: Vec<f32> = (0..48).map(|i| (i as f32) * 0.1).collect();
    let block_size = 16;
    let fft_size = 32;
    let parts = IrPartitions::from_ir(&ir, block_size);
    let fft = RealPackedFft::new(fft_size);

    for i in 0..parts.num_partitions() {
        let mut buf = parts.partitions[i].to_vec();
        fft.inverse(&mut buf);
        let start = i * block_size;
        let end = (start + block_size).min(ir.len());
        for j in 0..block_size {
            let expected = if start + j < end { ir[start + j] } else { 0.0 };
            assert!(
                (buf[j] - expected).abs() < 1e-3,
                "partition {i} sample {j}: got {} expected {expected}",
                buf[j],
            );
        }
    }
}
