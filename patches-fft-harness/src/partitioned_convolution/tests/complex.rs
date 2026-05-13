use super::*;

#[test]
fn cma_dc_and_nyquist() {
    let a = [2.0, 3.0, 0.0, 0.0];
    let b = [4.0, 5.0, 0.0, 0.0];
    let mut acc = [0.0; 4];
    complex_multiply_accumulate_packed(&mut acc, &a, &b);
    assert_eq!(acc[0], 8.0); // 2*4
    assert_eq!(acc[1], 15.0); // 3*5
}

#[test]
fn cma_interior_bins() {
    // Single interior bin: a = 3+4i, b = 1+2i => (3+4i)(1+2i) = -5+10i
    let a = [0.0, 0.0, 3.0, 4.0];
    let b = [0.0, 0.0, 1.0, 2.0];
    let mut acc = [0.0; 4];
    complex_multiply_accumulate_packed(&mut acc, &a, &b);
    assert!((acc[2] - (-5.0)).abs() < 1e-6);
    assert!((acc[3] - 10.0).abs() < 1e-6);
}

#[test]
fn cma_accumulates() {
    let a = [1.0, 1.0, 1.0, 0.0];
    let b = [2.0, 3.0, 2.0, 0.0];
    let mut acc = [10.0, 20.0, 30.0, 40.0];
    complex_multiply_accumulate_packed(&mut acc, &a, &b);
    assert_eq!(acc[0], 12.0); // 10 + 1*2
    assert_eq!(acc[1], 23.0); // 20 + 1*3
    assert_eq!(acc[2], 32.0); // 30 + 1*2 - 0*0
    assert_eq!(acc[3], 40.0); // 40 + 1*0 + 0*2
}

#[test]
fn complex_multiply_packed_basic() {
    let a = [2.0, 3.0, 1.0, 2.0, 3.0, 4.0];
    let b = [4.0, 5.0, 5.0, 6.0, 1.0, 2.0];
    let mut out = [0.0; 6];
    complex_multiply_packed(&mut out, &a, &b);
    assert_eq!(out[0], 8.0);
    assert_eq!(out[1], 15.0);
    // (1+2i)(5+6i) = 5+6i+10i-12 = -7+16i
    assert!((out[2] - (-7.0)).abs() < 1e-6);
    assert!((out[3] - 16.0).abs() < 1e-6);
    // (3+4i)(1+2i) = 3+6i+4i-8 = -5+10i
    assert!((out[4] - (-5.0)).abs() < 1e-6);
    assert!((out[5] - 10.0).abs() < 1e-6);
}
