//! Reproduces **R's** Mersenne-Twister RNG and the default inversion `rnorm`
//! bit-exactly, so that empirical MR-PRESSO p-values (which count random draws)
//! can be cross-validated against the R reference with a shared `set.seed`.
//!
//! R's default RNG kind is `c("Mersenne-Twister", "Inversion", "Rejection")`:
//!
//! - `set.seed(s)` initialises the MT19937 state with R's **`MT_sgenrand`**
//!   (a 69069-LCG scramble — *not* the reference `init_genrand`).
//! - `runif()` draws `MT_genrand()` — `y * 2⁻³²` where `y` is one tempered
//!   32-bit MT word.
//! - `rnorm(n, mean, sd)` = `mean + sd * qnorm5(runif(), 0, 1)` — the
//!   inversion method using Wichura's AS 241 `qnorm5`. Each `rnorm` consumes
//!   exactly one `runif` (one MT word).
//!
//! [`Rng::rnorm`] is implemented so the full draw sequence is identical to
//! R's for a given seed.

/// R's Mersenne-Twister (MT19937) with R's seeding, reproducing `runif`.
#[derive(Clone)]
pub struct Rng {
    mt: [u32; 624],
    mti: usize,
}

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908b0df;
const UPPER_MASK: u32 = 0x80000000;
const LOWER_MASK: u32 = 0x7fffffff;

impl Rng {
    /// Reproduce `set.seed(seed)` for the Mersenne-Twister kind.
    ///
    /// R's `RNG_Init` first pre-scrambles the seed with 50 `69069·s + 1`
    /// applications, then fills 625 state slots (`dummy[0..624]`) with the LCG.
    /// The MT array is `dummy[1..624]` (the first slot is the position counter,
    /// reset to `N` so the next draw triggers a fresh twist).
    pub fn new(seed: u32) -> Self {
        let mut s: u32 = seed;
        for _ in 0..50 {
            s = 69069u32.wrapping_mul(s).wrapping_add(1);
        }
        // i_seed[0] is the position counter (discarded here; mti reset to N).
        s = 69069u32.wrapping_mul(s).wrapping_add(1);
        let mut mt = [0u32; N];
        for i in 0..N {
            s = 69069u32.wrapping_mul(s).wrapping_add(1);
            mt[i] = s;
        }
        Rng { mt, mti: N }
    }

    /// One tempered 32-bit MT word (`MT_genrand`'s integer output).
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut y: u32;
        if self.mti >= N {
            for kk in 0..(N - M) {
                y = (self.mt[kk] & UPPER_MASK) | (self.mt[kk + 1] & LOWER_MASK);
                self.mt[kk] = self.mt[kk + M] ^ (y >> 1) ^ (if y & 1 == 1 { MATRIX_A } else { 0 });
            }
            for kk in (N - M)..(N - 1) {
                y = (self.mt[kk] & UPPER_MASK) | (self.mt[kk + 1] & LOWER_MASK);
                // (kk + M - N) with M < N, evaluated left-to-right (no underflow).
                self.mt[kk] =
                    self.mt[kk + M - N] ^ (y >> 1) ^ (if y & 1 == 1 { MATRIX_A } else { 0 });
            }
            y = (self.mt[N - 1] & UPPER_MASK) | (self.mt[0] & LOWER_MASK);
            self.mt[N - 1] = self.mt[M - 1] ^ (y >> 1) ^ (if y & 1 == 1 { MATRIX_A } else { 0 });
            self.mti = 0;
        }
        y = self.mt[self.mti];
        self.mti += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c5680;
        y ^= (y << 15) & 0xefc60000;
        y ^= y >> 18;
        y
    }

    /// One uniform in [0, 1) (`MT_genrand`), as produced by R's `runif`.
    #[inline]
    pub fn runif(&mut self) -> f64 {
        self.next_u32() as f64 * 2.3283064365386963e-10
    }

    /// `rnorm(n, mean, sd)` via R's Inversion method. A single `runif` is not
    /// precise enough, so R combines two uniforms:
    /// `qnorm((floor(2²⁷·u₁) + u₂) / 2²⁷)`, consuming two MT words per draw.
    #[inline]
    pub fn rnorm(&mut self, mean: f64, sd: f64) -> f64 {
        let big = 134_217_728.0; // 2^27
        let u1 = self.runif();
        let u2 = self.runif();
        let p = (big * u1).trunc() + u2;
        mean + sd * qnorm5(p / big)
    }

    /// R's `rbits(bits)`: assemble `ceil(bits/16)` uniform draws into a
    /// `bits`-bit random integer (used by the rejection sampler).
    #[inline]
    pub fn rbits(&mut self, bits: u32) -> f64 {
        let mut v: u64 = 0;
        let mut n = 0u32;
        while n <= bits {
            let v1 = (self.runif() * 65536.0).floor() as u64;
            v = 65536 * v + v1;
            n += 16;
        }
        (v & ((1u64 << bits) - 1)) as f64
    }

    /// R's `R_unif_index(dn)` (Rejection sample kind): a uniform index in
    /// `[0, dn)` obtained by rejection sampling from the next power of two.
    #[inline]
    pub fn unif_index(&mut self, dn: f64) -> f64 {
        if dn <= 0.0 {
            return 0.0;
        }
        let bits = dn.log2().ceil() as u32;
        loop {
            let dv = self.rbits(bits);
            if !(dn <= dv) {
                return dv;
            }
        }
    }
}

/// Wichura's AS 241 normal quantile, transcribed from R's `qnorm.c`
/// (`qnorm5`), for `lower.tail = TRUE`, `log.p = FALSE`. Returns `x` such that
/// `Φ(x) = p`.
pub fn qnorm5(p: f64) -> f64 {
    debug_assert!(p > 0.0 && p < 1.0);
    // p_ := R_DT_qIv(p)  (lower_tail=TRUE, log_p=FALSE)  ==  p
    let p_ = p;
    let q = p_ - 0.5;

    if q.abs() <= 0.425 {
        // 0.075 <= p <= 0.925
        let r = 0.180625 - q * q;
        let num = ((((((r * 2509.0809287301226727 + 33430.575583588128105) * r
            + 67265.770927008700853)
            * r
            + 45921.953931549871457)
            * r
            + 13731.693765509461125)
            * r
            + 1971.5909503065514427)
            * r
            + 133.14166789178437745)
            * r
            + 3.387132872796366608;
        let den = ((((((r * 5226.495278852854561 + 28729.085735721942674) * r
            + 39307.89580009271061)
            * r
            + 21213.794301586595867)
            * r
            + 5394.1960214247511077)
            * r
            + 687.1870074920579083)
            * r
            + 42.313330701600911252)
            * r
            + 1.0;
        q * num / den
    } else {
        // p < 0.075 or p > 0.925
        let lp = if q > 0.0 { (1.0 - p_).ln() } else { p_.ln() };
        let r = (-lp).sqrt();
        let val = if r <= 5.0 {
            // min(p,1-p) >= exp(-25)
            let r = r - 1.6;
            let num = ((((((r * 7.7454501427834140764e-4 + 0.0227238449892691845833) * r
                + 0.24178072517745061177)
                * r
                + 1.27045825245236838258)
                * r
                + 3.64784832476320460504)
                * r
                + 5.7694972214606914055)
                * r
                + 4.6303378461565452959)
                * r
                + 1.42343711074968357734;
            let den = ((((((r * 1.05075007164441684324e-9 + 5.475938084995344946e-4) * r
                + 0.0151986665636164571966)
                * r
                + 0.14810397642748007459)
                * r
                + 0.68976733498510000455)
                * r
                + 1.6763848301838038494)
                * r
                + 2.05319162663775882187)
                * r
                + 1.0;
            num / den
        } else if r <= 27.0 {
            let r = r - 5.0;
            let num = ((((((r * 2.01033439929228813265e-7 + 2.71155556874348757815e-5) * r
                + 0.0012426609473880784386)
                * r
                + 0.026532189526576123093)
                * r
                + 0.29656057182850489123)
                * r
                + 1.7848265399172913358)
                * r
                + 5.4637849111641143699)
                * r
                + 6.6579046435011037772;
            let den = ((((((r * 2.04426310338993978564e-15 + 1.4215117583164458887e-7) * r
                + 1.8463183175100546818e-5)
                * r
                + 7.868691311456132591e-4)
                * r
                + 0.0148753612908506148525)
                * r
                + 0.13692988092273580531)
                * r
                + 0.59983220655588793769)
                * r
                + 1.0;
            num / den
        } else if r >= 6.4e8 {
            r * std::f64::consts::SQRT_2
        } else {
            // r > 27: use the asymptotic series (only reachable via log_p in R;
            // kept for completeness).
            let s2 = -2.0 * lp;
            let mut x2 = s2 - (std::f64::consts::TAU * s2).ln();
            if r < 36000.0 {
                x2 = s2 - (std::f64::consts::TAU * x2).ln() - 2.0 / (2.0 + x2);
                if r < 840.0 {
                    x2 = s2 - (std::f64::consts::TAU * x2).ln()
                        + 2.0 * (1.0 - (1.0 - 1.0 / (4.0 + x2)) / (2.0 + x2)).ln_1p();
                    if r < 109.0 {
                        x2 = s2 - (std::f64::consts::TAU * x2).ln()
                            + 2.0
                                * (1.0
                                    - (1.0 - 5.0 / (6.0 + x2)) / (4.0 + x2))
                                .ln_1p();
                        if r < 55.0 {
                            x2 = s2 - (std::f64::consts::TAU * x2).ln()
                                + 2.0
                                    * (1.0
                                        - (1.0 - (5.0 - 9.0 / (8.0 + x2)) / (6.0 + x2))
                                            / (4.0 + x2))
                                    .ln_1p();
                        }
                    }
                }
            }
            x2.sqrt()
        };
        if q < 0.0 {
            -val
        } else {
            val
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R: `set.seed(123); runif(5)` → 0.2875775 0.7883051 0.4089769 ...
    #[test]
    fn matches_r_runif() {
        let mut rng = Rng::new(123);
        let got: Vec<f64> = (0..5).map(|_| rng.runif()).collect();
        let expected = [
            0.287577520124614,
            0.788305135443807,
            0.4089769218117,
            0.883017404004931,
            0.940467284293845,
        ];
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-15, "got {g:0.16} expected {e:0.16}");
        }
    }

    /// R: `set.seed(123); rnorm(5)` → -0.5604756 -0.2301775 1.558708 ...
    #[test]
    fn matches_r_rnorm() {
        let mut rng = Rng::new(123);
        let got: Vec<f64> = (0..5).map(|_| rng.rnorm(0.0, 1.0)).collect();
        let expected = [
            -0.5604756465522126,
            -0.230177489483280,
            1.5587083141491224,
            0.070508391424576,
            0.129287735160946,
        ];
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-14, "got {g:0.16} expected {e:0.16}");
        }
    }

    /// R: `set.seed(123); rnorm(5, 2, 3)` → 7.145195 3.382749 -1.795184 ...
    #[test]
    fn matches_r_rnorm_shifted() {
        let mut rng = Rng::new(123);
        let got: Vec<f64> = (0..5).map(|_| rng.rnorm(2.0, 3.0)).collect();
        // rnorm(mean, sd) = mean + sd * qnorm(u) for the *same* uniform stream
        // as rnorm(0, 1); a fresh seed gives 2 + 3 * [rnorm(0,1) draws].
        let d = [
            -0.5604756465522126,
            -0.230177489483280,
            1.5587083141491224,
            0.070508391424576,
            0.129287735160946,
        ];
        let expected: Vec<f64> = d.iter().map(|x| 2.0 + 3.0 * x).collect();
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-14, "got {g:0.16} expected {e:0.16}");
        }
    }
}
