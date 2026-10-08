// SPDX-License-Identifier: GPL-3.0
//
// Ported from KolmogorovSmirnovDist.c, version 1.1 of 1 February 2012, by Richard Simard (DIRO, Université de
// Montréal), which carries this notice:
//
//   Copyright 1 march 2010 by Université de Montréal, Richard Simard and Pierre L'Ecuyer
//
//   This program is free software: you can redistribute it and/or modify it under the terms of the GNU General
//   Public License as published by the Free Software Foundation, version 3 of the License.
//
//   This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the
//   implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU General Public License
//   for more details.
//
//   You should have received a copy of the GNU General Public License along with this program.  If not, see
//   <http://www.gnu.org/licenses/>.

//! The Kolmogorov-Smirnov distribution (`src/web/api/queries/KolmogorovSmirnovDist.c`): [`ks_fbar`] is the
//! probability that the statistic of a sample of `n` exceeds `x`, which the `ks2` weights method turns into a
//! metric's weight.
//!
//! The port keeps C's arithmetic operation by operation, in C's order, and calls `f64`'s `exp`, `ln`, `ln_1p`,
//! `powf`, `sqrt` and `ceil` where C calls libm: on glibc these are the functions C calls, and the answers are C's
//! bit for bit (the unit `ks_fbar_answers_as_c`, on a vector C made). Nothing here may be "simplified": `powi` for
//! `powf`, `ln(1.0 + x)` for `ln_1p(x)`, a fused multiply-add or a reordered sum each change bits.
//!
//! C's only caller, `ks_2samp()` in `weights.c`, passes `n >= 1` and a finite `x` in `[0, 1]`. Outside that: any
//! `n < 1` gives 1 or 0 as in C, and a NaN `x`, on which C reads outside a table, gives NaN.

use std::f64::consts::{LN_2, PI};

/// `NEXACT`: up to this sample size the exact algorithms answer (Durbin's matrix, Pomeranz), above it the
/// asymptotic ones, except near 0, where Durbin's matrix still answers up to [`NKOLMO`].
const NEXACT: i32 = 500;
/// `NKOLMO`.
const NKOLMO: i32 = 100_000;

/// `MFACT`: the last sample size [`LN_FACTORIAL`] holds.
const MFACT: i32 = 30;

/// `LnFactorial[]`: the natural logarithm of n! for n up to [`MFACT`]. C's third entry is `0.6931471805599453`.
const LN_FACTORIAL: [f64; MFACT as usize + 1] = [
    0.,
    0.,
    LN_2,
    1.791759469228055,
    3.178053830347946,
    4.787491742782046,
    6.579251212010101,
    8.525161361065415,
    10.60460290274525,
    12.80182748008147,
    15.10441257307552,
    17.50230784587389,
    19.98721449566188,
    22.55216385312342,
    25.19122118273868,
    27.89927138384088,
    30.67186010608066,
    33.50507345013688,
    36.39544520803305,
    39.33988418719949,
    42.33561646075348,
    45.3801388984769,
    48.47118135183522,
    51.60667556776437,
    54.7847293981123,
    58.00360522298051,
    61.26170176100199,
    64.55753862700632,
    67.88974313718154,
    71.257038967168,
    74.65823634883016,
];

/// `getLogFactorial()`: ln(n!), from the table or by Stirling's series.
fn log_factorial(n: i32) -> f64 {
    if n <= MFACT {
        return LN_FACTORIAL[n as usize];
    }
    let x = f64::from(n + 1);
    let y = 1.0 / (x * x);
    let z = ((-(5.95238095238E-4 * y) + 7.936500793651E-4) * y - 2.7777777777778E-3) * y + 8.3333333333333E-2;
    ((x - 0.5) * x.ln() - x) + 9.1893853320467E-1 + z / x
}

/// `rapfac()`: n! / n^n.
fn rapfac(n: i32) -> f64 {
    let mut res = 1.0 / f64::from(n);
    for i in 2..=n {
        res *= f64::from(i) / f64::from(n);
    }
    res
}

/// `KSPlusbarAsymp()`: the upper tail of the one-sided statistic by an asymptotic formula.
fn plusbar_asymp(n: i32, x: f64) -> f64 {
    let n = f64::from(n);
    let t = 6.0 * n * x + 1.0;
    let z = t * t / (18.0 * n);
    let v = 1.0 - (2.0 * z * z - 4.0 * z - 1.0) / (18.0 * n);
    if v <= 0.0 {
        return 0.0;
    }
    let v = v * (-z).exp();
    if v >= 1.0 {
        return 1.0;
    }
    v
}

/// `KSPlusbarUpper()`: the upper tail of the one-sided statistic by Smirnov's stable formula. The sum runs up
/// from a third (or, for a large sample, a half) of its last term and then down from there, each way until a term
/// no longer counts.
fn plusbar_upper(n: i32, x: f64) -> f64 {
    const EPSILON: f64 = 1.0E-12;
    if n > 200_000 {
        return plusbar_asymp(n, x);
    }
    let nf = f64::from(n);
    let mut jmax = (nf * (1.0 - x)) as i32;
    // avoid log(0) for j = jmax and q ~ 1.0
    if (1.0 - x - f64::from(jmax) / nf) <= 0.0 {
        jmax -= 1;
    }
    let jdiv = if n > 3000 { 2 } else { 3 };
    let term = |log_com: f64, j: i32| {
        let q = f64::from(j) / nf + x;
        (log_com + f64::from(j - 1) * q.ln() + f64::from(n - j) * (-q).ln_1p()).exp()
    };
    let mut sum = 0.0;

    let first = jmax / jdiv + 1;
    let log_jmax = log_factorial(n) - log_factorial(first) - log_factorial(n - first);
    let mut log_com = log_jmax;
    for j in first..=jmax {
        let t = term(log_com, j);
        sum += t;
        log_com += (f64::from(n - j) / f64::from(j + 1)).ln();
        if t <= sum * EPSILON {
            break;
        }
    }

    let first = jmax / jdiv;
    let mut log_com = log_jmax + (f64::from(first + 1) / f64::from(n - first)).ln();
    for j in (1..=first).rev() {
        let t = term(log_com, j);
        sum += t;
        log_com += (f64::from(j) / f64::from(n - j + 1)).ln();
        if t <= sum * EPSILON {
            break;
        }
    }

    sum *= x;
    // the term j = 0
    sum + (nf * (-x).ln_1p()).exp()
}

/// One of the six series of [`pelz`]: the terms from index `first` on, added while the last one still counts
/// against the sum (by magnitude where `signed`), to index 20 at the most. The first term always counts.
fn pelz_series(first: i32, signed: bool, term_at: impl Fn(i32) -> f64) -> f64 {
    const JMAX: i32 = 20;
    const EPS: f64 = 1.0e-10;
    let counts = |term: f64, sum: f64| if signed { term.abs() > EPS * sum.abs() } else { term > EPS * sum };
    let (mut term, mut sum) = (1.0, 0.0);
    let mut j = first;
    while j <= JMAX && counts(term, sum) {
        term = term_at(j);
        sum += term;
        j += 1;
    }
    sum
}

/// `Pelz()`: the lower tail by Pelz and Good's asymptotic series (Journal of the Royal Statistical Society B 38,
/// 1976).
fn pelz(n: i32, x: f64) -> f64 {
    // sqrt(2*Pi) and sqrt(Pi/2), as C writes them
    const C: f64 = 2.506628274631001;
    const C2: f64 = 1.2533141373155001;
    const PI2: f64 = PI * PI;
    const PI4: f64 = PI2 * PI2;
    let nf = f64::from(n);
    let racn = nf.sqrt();
    let z = racn * x;
    let z2 = z * z;
    let z4 = z2 * z2;
    let z6 = z4 * z2;
    let w = PI2 / (2.0 * z * z);
    let half = |j: i32| f64::from(j) + 0.5;

    let mut sum = pelz_series(0, false, |j| {
        let ti = half(j);
        (-ti * ti * w).exp()
    });
    sum *= C / z;

    let tom = pelz_series(0, true, |j| {
        let ti = half(j);
        (PI2 * ti * ti - z2) * (-ti * ti * w).exp()
    });
    sum += tom * C2 / (racn * 3.0 * z4);

    let tom = pelz_series(0, true, |j| {
        let ti = half(j);
        let term = 6.0 * z6 + 2.0 * z4
            + PI2 * (2.0 * z4 - 5.0 * z2) * ti * ti
            + PI4 * (1.0 - 2.0 * z2) * ti * ti * ti * ti;
        term * (-ti * ti * w).exp()
    });
    sum += tom * C2 / (nf * 36.0 * z * z6);

    let tom = pelz_series(1, false, |j| {
        let ti = f64::from(j);
        PI2 * ti * ti * (-ti * ti * w).exp()
    });
    sum -= tom * C2 / (nf * 18.0 * z * z2);

    let tom = pelz_series(0, true, |j| {
        let ti = half(j);
        let ti = ti * ti;
        let term = -30.0 * z6 - 90.0 * z6 * z2
            + PI2 * (135.0 * z4 - 96.0 * z6) * ti
            + PI4 * (212.0 * z4 - 60.0 * z2) * ti * ti
            + PI2 * PI4 * ti * ti * ti * (5.0 - 30.0 * z2);
        term * (-ti * w).exp()
    });
    sum += tom * C2 / (racn * nf * 3240.0 * z4 * z6);

    let tom = pelz_series(1, true, |j| {
        let ti = f64::from(j * j);
        (3.0 * PI2 * ti * z2 - PI4 * ti * ti) * (-ti * w).exp()
    });
    sum += tom * C2 / (racn * nf * 108.0 * z6);

    sum
}

/// What `CalcFloorCeil()` precomputes for [`pomeranz`]: the `A_i`, and the limits of its sums, `floor(A_i - t)` and
/// `ceil(A_i + t)`, for `i` up to `2n + 2`. Index 0 of the limits is unused, as in C.
struct PomeranzLimits {
    a: Vec<f64>,
    floors: Vec<i32>,
    ceils: Vec<i32>,
}

/// `CalcFloorCeil()`, for `t = n * x`. C keeps the limits as doubles holding whole numbers and casts them back;
/// here they stay integers.
fn pomeranz_limits(n: i32, t: f64) -> PomeranzLimits {
    let top = 2 * n as usize + 2;
    let ell = t as i32;
    let z = t - f64::from(ell);
    let w = t.ceil() - t;
    let mut floors = vec![0; top + 1];
    let mut ceils = vec![0; top + 1];
    for i in 1..=top {
        let half = (i / 2) as i32;
        let even = (i & 1) == 0;
        (floors[i], ceils[i]) = if z > 0.5 {
            if even { (half - 2 - ell, half + ell) } else { (half - 1 - ell, half + 1 + ell) }
        } else if z > 0.0 {
            (half - 1 - ell, if i == 1 { 1 + ell } else { half + ell })
        } else if even {
            (half - 1 - ell, half - 1 + ell)
        } else {
            (half - ell, half + ell)
        };
    }

    let mut a = vec![0.0; top + 1];
    a[2] = if w < z { w } else { z };
    a[3] = 1.0 - a[2];
    for i in 4..top {
        a[i] = a[i - 2] + 1.0;
    }
    a[top] = f64::from(n);
    PomeranzLimits { a, floors, ceils }
}

/// `Pomeranz()`: the distribution by Pomeranz's recursion, exact, for a sample of 500 at the most.
fn pomeranz(n: i32, x: f64) -> f64 {
    const EPS: f64 = 1.0e-15;
    const ENO: i32 = 350;
    // ldexp(1.0, ENO): what the rows are multiplied by when they get too small
    const RENO: f64 = f64::from_bits((1023 + ENO as u64) << 52);
    let nf = f64::from(n);
    let columns = n as usize + 2;
    let PomeranzLimits { a, floors, ceils } = pomeranz_limits(n, nf * x);

    // V[i][] and V[i-1][], and how many times they were renormalized
    let mut v = [vec![0.0; columns], vec![0.0; columns]];
    v[1][1] = RENO;
    let mut coreno = 1;

    // H[][j] = w^j / j! for each of the four values w = (A[i] - A[i-1]) / n can take
    let powers = |w: f64| {
        let mut row = vec![1.0; columns];
        for j in 1..columns {
            row[j] = w * row[j - 1] / j as f64;
        }
        row
    };
    let h = [powers(2.0 * a[2] / nf), powers((1.0 - 2.0 * a[2]) / nf), powers(a[2] / nf), powers(0.0)];

    let (mut r1, mut r2) = (0, 1);
    for i in 2..=2 * n as usize + 2 {
        let jlow = (2 + floors[i]).max(1);
        let jup = ceils[i].min(n + 1);
        let klow = (2 + floors[i - 1]).max(1);
        let kup0 = ceils[i - 1];

        // which of the four values this step's w is. C leaves the row's index at -1 when none is and reads before
        // its table; the difference is one of the four by construction (C's commented-out assert)
        let w = (a[i] - a[i - 1]) / nf;
        let Some(hs) = h.iter().find(|row| (w - row[1]).abs() <= EPS) else {
            return f64::NAN;
        };

        let mut minsum = RENO;
        r1 = (r1 + 1) & 1;
        r2 = (r2 + 1) & 1;
        for j in jlow..=jup {
            let mut sum = 0.0;
            for k in (klow..=kup0.min(j)).rev() {
                sum += v[r1][k as usize] * hs[(j - k) as usize];
            }
            v[r2][j as usize] = sum;
            if sum < minsum {
                minsum = sum;
            }
        }

        if minsum < 1.0e-280 {
            // V is too small: renormalize to avoid underflow of probabilities
            for j in jlow..=jup {
                v[r2][j as usize] *= RENO;
            }
            coreno += 1;
        }
    }

    let sum = v[r2][n as usize + 1];
    let w = log_factorial(n) - f64::from(coreno * ENO) * LN_2 + sum.ln();
    if w >= 0. {
        return 1.;
    }
    w.exp()
}

/// `cdfSpecial()`: the distribution where it is known exactly, else -1.
fn cdf_special(n: i32, x: f64) -> f64 {
    let nf = f64::from(n);
    // for n x^2 > 18 the upper tail is smaller than 5e-16
    if nf * x * x >= 18.0 || x >= 1.0 {
        return 1.0;
    }
    if x <= 0.5 / nf {
        return 0.0;
    }
    if n == 1 {
        return 2.0 * x - 1.0;
    }
    if x <= 1.0 / nf {
        let t = 2.0 * x * nf - 1.0;
        if n <= NEXACT {
            return rapfac(n) * t.powf(nf);
        }
        return (log_factorial(n) + nf * (t / nf).ln()).exp();
    }
    if x >= 1.0 - 1.0 / nf {
        return 1.0 - 2.0 * (1.0 - x).powf(nf);
    }
    -1.0
}

/// `KScdf()`: the probability that the statistic of a sample of `n` is below `x`. C exports it; only `KSfbar()`
/// is called, and it screens the sample sizes and statistics this function cannot take.
fn ks_cdf(n: i32, x: f64) -> f64 {
    let nf = f64::from(n);
    let w = nf * x * x;
    let u = cdf_special(n, x);
    if u >= 0.0 {
        return u;
    }
    if n <= NEXACT {
        if w < 0.754693 {
            return durbin_matrix(n, x);
        }
        if w < 4.0 {
            return pomeranz(n, x);
        }
        return 1.0 - ks_fbar(n, x);
    }
    if w * x * nf <= 7.0 && n <= NKOLMO {
        return durbin_matrix(n, x);
    }
    pelz(n, x)
}

/// `fbarSpecial()`: the complementary distribution where it is known exactly, else -1.
fn fbar_special(n: i32, x: f64) -> f64 {
    let nf = f64::from(n);
    let w = nf * x * x;
    if w >= 370.0 || x >= 1.0 {
        return 0.0;
    }
    if w <= 0.0274 || x <= 0.5 / nf {
        return 1.0;
    }
    if n == 1 {
        return 2.0 - 2.0 * x;
    }
    if x <= 1.0 / nf {
        let t = 2.0 * x * nf - 1.0;
        if n <= NEXACT {
            return 1.0 - rapfac(n) * t.powf(nf);
        }
        return 1.0 - (log_factorial(n) + nf * (t / nf).ln()).exp();
    }
    if x >= 1.0 - 1.0 / nf {
        return 2.0 * (1.0 - x).powf(nf);
    }
    -1.0
}

/// `KSfbar()`: the probability that the statistic of a sample of `n` exceeds `x`.
pub fn ks_fbar(n: i32, x: f64) -> f64 {
    // C reads outside its factorials' table for a NaN statistic; its caller never passes one
    if x.is_nan() {
        return f64::NAN;
    }
    let w = f64::from(n) * x * x;
    let v = fbar_special(n, x);
    if v >= 0.0 {
        return v;
    }
    if n <= NEXACT {
        if w < 4.0 {
            return 1.0 - ks_cdf(n, x);
        }
        return 2.0 * plusbar_upper(n, x);
    }
    if w >= 2.65 {
        return 2.0 * plusbar_upper(n, x);
    }
    1.0 - ks_cdf(n, x)
}

// The Durbin matrix algorithm, programmed by G. Marsaglia, Wai Wan Tsang and Jingbo Wong (J. Stat. Software 8, 18,
// 2003), with Richard Simard's small modifications:
//
//   K(n, d) = Prob(D_n < d), where D_n = max(x_1 - 0/n, x_2 - 1/n, ..., x_n - (n-1)/n, 1/n - x_1, ..., n/n - x_n)
//   with x_1 < x_2 < ... < x_n a purported set of n independent uniform [0, 1) random variables.

/// `NORM`, `INORM`, `LOGNORM`: a matrix whose middle element passes 1e140 is scaled down by it, and the power of
/// ten is kept apart.
const NORM: f64 = 1.0e140;
const INORM: f64 = 1.0e-140;
const LOGNORM: i32 = 140;

/// `DurbinMatrix()`. The matrices are `m` by `m`, stored by rows.
fn durbin_matrix(n: i32, d: f64) -> f64 {
    let nf = f64::from(n);
    let k = (nf * d) as i32 + 1;
    let h = f64::from(k) - nf * d;
    let k = k as usize;
    let m = 2 * k - 1;
    let mut hm = vec![0.0; m * m];
    for i in 0..m {
        for j in 0..m {
            hm[i * m + j] = if i + 1 < j { 0.0 } else { 1.0 };
        }
    }
    for i in 0..m {
        hm[i * m] -= h.powf((i + 1) as f64);
        hm[(m - 1) * m + i] -= h.powf((m - i) as f64);
    }
    hm[(m - 1) * m] += if 2.0 * h - 1.0 > 0.0 { (2.0 * h - 1.0).powf(m as f64) } else { 0.0 };
    for i in 0..m {
        for j in 0..m {
            // empty where j is past i + 1
            for g in 1..=(i + 1).saturating_sub(j) {
                hm[i * m + j] /= g as f64;
            }
        }
    }

    let (q, mut eq) = matrix_power(&hm, m, n);
    let mut s = q[(k - 1) * m + k - 1];
    for i in 1..=n {
        s = s * f64::from(i) / nf;
        if s < INORM {
            s *= NORM;
            eq -= LOGNORM;
        }
    }
    s * 10f64.powf(f64::from(eq))
}

/// `mMultiply()`: the product of two `m` by `m` matrices.
fn matrix_multiply(a: &[f64], b: &[f64], m: usize) -> Vec<f64> {
    let mut c = vec![0.0; m * m];
    for i in 0..m {
        for j in 0..m {
            let mut s = 0.;
            for k in 0..m {
                s += a[i * m + k] * b[k * m + j];
            }
            c[i * m + j] = s;
        }
    }
    c
}

/// `renormalize()`: scales the matrix down and counts the power of ten in `exponent`.
fn renormalize(v: &mut [f64], exponent: &mut i32) {
    for x in v {
        *x *= INORM;
    }
    *exponent += LOGNORM;
}

/// `mPower()`: `a` to the power `n` by squaring, as a matrix and the power of ten it was scaled down by. `a`
/// itself carries none (C's `eA` is 0 at its one call).
fn matrix_power(a: &[f64], m: usize, n: i32) -> (Vec<f64>, i32) {
    if n == 1 {
        return (a.to_vec(), 0);
    }
    let (half, half_exponent) = matrix_power(a, m, n / 2);
    let mut b = matrix_multiply(&half, &half, m);
    let mut eb = 2 * half_exponent;
    if b[(m / 2) * m + (m / 2)] > NORM {
        renormalize(&mut b, &mut eb);
    }
    let (mut v, mut ev) = if n % 2 == 0 { (b, eb) } else { (matrix_multiply(a, &b, m), eb) };
    if v[(m / 2) * m + (m / 2)] > NORM {
        renormalize(&mut v, &mut ev);
    }
    (v, ev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weights::vector::{Vector, double};

    /// [`ks_fbar`] against the answers C's `KSfbar()` gave (`tests/oracle/gen-ks-vectors.c`, which includes the
    /// production file unchanged), for pairs of a sample size and a statistic spread over every method the function
    /// selects: every limit of the function two doubles either side, for sample sizes on both sides of each limit
    /// on the size; a grid around 500, where the exact methods end; and what `ks_2samp()` can ask. Bit for bit on
    /// the vector's class of machine ([`Vector`]).
    #[test]
    fn ks_fbar_answers_as_c() {
        let vector = Vector::read("ksfbar.txt");
        for line in &vector.lines {
            let mut fields = line.split(' ');
            let n: i32 = fields.next().unwrap().parse().unwrap();
            let (x, expected) = (double(fields.next().unwrap()), double(fields.next().unwrap()));
            vector.assert_as_c(ks_fbar(n, x), expected, format_args!("KSfbar({n}, {x:e})"));
        }
        assert!(vector.lines.len() > 3000, "{} answers", vector.lines.len());
        vector.report("ksfbar", vector.lines.len());
    }

    /// The two constants C writes as literals are the standard library's, bit for bit, and the third entry of the
    /// factorials' table is C's literal.
    #[test]
    fn the_constants_are_c_s_literals() {
        let parsed = |text: &str| text.parse::<f64>().unwrap().to_bits();
        assert_eq!(PI.to_bits(), parsed("3.14159265358979323846"));
        assert_eq!(LN_2.to_bits(), parsed("0.69314718055994530941"));
        assert_eq!(LN_FACTORIAL[2].to_bits(), parsed("0.6931471805599453"));
        assert_eq!(LN_FACTORIAL.len(), 31);
    }

    /// Where the distribution is known exactly no libm call is made, so these hold on every machine: the tail is
    /// 0 from `n x^2 >= 370` or `x >= 1` on, 1 up to `n x^2 <= 0.0274` or `x <= 0.5 / n`, and `2 - 2x` for one
    /// sample. A sample size below 1 lands in the first two; a NaN statistic gives NaN.
    #[test]
    fn the_exact_cases_need_no_libm() {
        let cases: [(i32, f64, f64); 12] = [
            (10, 1.0, 0.0),
            (10, 6.1, 0.0),
            (1000, 0.61, 0.0),
            (10, 0.05, 1.0),
            (10, 0.0, 1.0),
            (10, -3.0, 1.0),
            (100, 0.0165, 1.0),
            (1, 0.75, 0.5),
            (1, 0.625, 0.75),
            (0, 0.5, 1.0),
            (-4, 0.5, 1.0),
            (0, 1.0, 0.0),
        ];
        for (n, x, expected) in cases {
            assert_eq!(ks_fbar(n, x).to_bits(), expected.to_bits(), "KSfbar({n}, {x})");
        }
        for n in [-4, 0, 1, 2, 500, 501, 5000] {
            assert!(ks_fbar(n, f64::NAN).is_nan(), "{n}");
        }
    }
}
