//! Sample-size maths for the power gate.

const A: [f64; 6] = [
    -3.969683028665376e+01,
    2.209460984245205e+02,
    -2.759285104469687e+02,
    1.383_577_518_672_69e2,
    -3.066479806614716e+01,
    2.506628277459239e+00,
];
const B: [f64; 5] = [
    -5.447609879822406e+01,
    1.615858368580409e+02,
    -1.556989798598866e+02,
    6.680131188771972e+01,
    -1.328068155288572e+01,
];
const C: [f64; 6] = [
    -7.784894002430293e-03,
    -3.223964580411365e-01,
    -2.400758277161838e+00,
    -2.549732539343734e+00,
    4.374664141464968e+00,
    2.938163982698783e+00,
];
const D: [f64; 4] = [
    7.784695709041462e-03,
    3.224671290700398e-01,
    2.445134137142996e+00,
    3.754408661907416e+00,
];
const P_LOW: f64 = 0.02425;

/// Horner's rule: `c[0] x^(n-1) + ... + c[n-1]`.
fn poly(c: &[f64], x: f64) -> f64 {
    c.iter().fold(0.0, |acc, k| acc * x + k)
}

/// Inverse standard normal CDF. Acklam's algorithm without the Newton step (std has no erfc); relative error about 1.15e-9.
/// `p <= 0` gives -inf, `p >= 1` gives +inf, NaN gives NaN.
pub fn z(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    let tail = |q: f64| poly(&C, q) / (poly(&D, q) * q + 1.0);
    if p < P_LOW {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - P_LOW {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        poly(&A, r) * q / (poly(&B, r) * r + 1.0)
    }
}

/// Units needed for a two-sided paired-mean test, with a design effect for clusters:
/// `ceil(((z(1 - alpha/2) + z(power)) * sd / sesoi)^2 * (1 + (m - 1) * icc))`.
pub fn required_units(
    alpha: f64,
    power: f64,
    sd: f64,
    sesoi: f64,
    units_per_cluster: u32,
    icc: f64,
) -> u64 {
    let base = ((z(1.0 - alpha / 2.0) + z(power)) * sd / sesoi).powi(2);
    (base * (1.0 + (f64::from(units_per_cluster) - 1.0) * icc)).ceil() as u64
}
