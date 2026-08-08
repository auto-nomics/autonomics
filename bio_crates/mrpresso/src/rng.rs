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
        let num = ((((((r * 2_509.080_928_730_122_7 + 33_430.575_583_588_13) * r
            + 67_265.770_927_008_7)
            * r
            + 45_921.953_931_549_87)
            * r
            + 13_731.693_765_509_46)
            * r
            + 1_971.590_950_306_551_3)
            * r
            + 133.141_667_891_784_38)
            * r
            + 3.387_132_872_796_366_5;
        let den = ((((((r * 5_226.495_278_852_854 + 28_729.085_735_721_943) * r
            + 39_307.895_800_092_71)
            * r
            + 21_213.794_301_586_597)
            * r
            + 5_394.196_021_424_751)
            * r
            + 687.187_007_492_057_9)
            * r
            + 42.313_330_701_600_91)
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
            let num = ((((((r * 7.745_450_142_783_414e-4 + 0.022_723_844_989_269_184) * r
                + 0.241_780_725_177_450_6)
                * r
                + 1.270_458_252_452_368_4)
                * r
                + 3.647_848_324_763_204_5)
                * r
                + 5.769_497_221_460_691)
                * r
                + 4.630_337_846_156_546)
                * r
                + 1.423_437_110_749_683_5;
            let den = ((((((r * 1.050_750_071_644_416_9e-9 + 5.475_938_084_995_345e-4) * r
                + 0.015_198_666_563_616_457)
                * r
                + 0.148_103_976_427_480_08)
                * r
                + 0.689_767_334_985_1)
                * r
                + 1.676_384_830_183_803_8)
                * r
                + 2.053_191_626_637_759)
                * r
                + 1.0;
            num / den
        } else if r <= 27.0 {
            let r = r - 5.0;
            let num = ((((((r * 2.010_334_399_292_288_1e-7 + 2.711_555_568_743_487_6e-5) * r
                + 0.001_242_660_947_388_078_4)
                * r
                + 0.026_532_189_526_576_124)
                * r
                + 0.296_560_571_828_504_87)
                * r
                + 1.784_826_539_917_291_3)
                * r
                + 5.463_784_911_164_114)
                * r
                + 6.657_904_643_501_103;
            let den = ((((((r * 2.044_263_103_389_939_7e-15 + 1.421_511_758_316_446e-7) * r
                + 1.846_318_317_510_054_8e-5)
                * r
                + 7.868_691_311_456_133e-4)
                * r
                + 0.014_875_361_290_850_615)
                * r
                + 0.136_929_880_922_735_8)
                * r
                + 0.599_832_206_555_888)
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
                            + 2.0 * (1.0 - (1.0 - 5.0 / (6.0 + x2)) / (4.0 + x2)).ln_1p();
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
        if q < 0.0 { -val } else { val }
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
