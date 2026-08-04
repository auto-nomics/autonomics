//! Bit-exact reproduction of **R 4.5.3**'s RNG infrastructure so that BKMR's
//! MCMC chains can be cross-validated against the reference `bkmr` R package
//! under a shared `set.seed`.
//!
//! R's default `RNGkind()` is `c("Mersenne-Twister", "Inversion",
//! "Rejection")`:
//!
//! - `set.seed(s)` initialises MT19937 via R's 69069-LCG scramble.
//! - `runif()` draws one tempered 32-bit MT word × 2⁻³².
//! - `rnorm(mean, sd)` uses the **Inversion** kind: Wichura's AS 241 `qnorm5`
//!   applied to `(floor(2²⁷·u₁) + u₂) / 2²⁷`, consuming two `runif` per draw.
//! - `rexp(scale)` uses **Ahrens–Dieter (1972)** `exp_rand` — a variable-draw
//!   rejection algorithm consuming 1 + 0..k `runif` calls.
//! - `rgamma(shape, scale)` uses Ahrens–Dieter **GS** (shape < 1) or **GD**
//!   (shape ≥ 1), drawing `norm_rand`, `unif_rand`, and `exp_rand`.
//! - `sample.int(n, 1)` uses `R_unif_index(n)`, which under the **Rejection**
//!   sample-kind draws ceil(log₂(n)/16) `runif` calls plus rejection retries.
//!
//! The `truncnorm::rtruncnorm` port reproduces the C source of truncnorm
//! 1.0-9 (Robert 1995 rejection), which calls `rnorm`/`runif`/`rexp`.
//!
//! Every public method consumes the *same* MT words that the corresponding R
//! function would, so a single [`Rng::new(seed)`] reproduces R's full draw
//! stream.

// ── MT19937 (R's seeding) ───────────────────────────────────────────────────

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908b0df;
const UPPER_MASK: u32 = 0x80000000;
const LOWER_MASK: u32 = 0x7fffffff;

/// R's Mersenne-Twister with R's `set.seed` scramble.
#[derive(Clone)]
pub struct Rng {
    mt: [u32; N],
    mti: usize,
}

impl Rng {
    /// Reproduce `set.seed(seed)` for the Mersenne-Twister kind.
    ///
    /// R's `RNG_Init` pre-scrambles the seed with 50 `69069·s + 1` steps,
    /// then fills 624 state slots with the same LCG. The position counter is
    /// reset to `N` so the next draw triggers a fresh twist.
    pub fn new(seed: u32) -> Self {
        let mut s: u32 = seed;
        for _ in 0..50 {
            s = 69069u32.wrapping_mul(s).wrapping_add(1);
        }
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

    /// One uniform in [0, 1) — R's `unif_rand()` / `runif()`.
    #[inline]
    pub fn runif(&mut self) -> f64 {
        self.next_u32() as f64 * 2.3283064365386963e-10
    }

    /// `rnorm(mean, sd)` via R's Inversion method: two uniforms combined and
    /// passed through Wichura's AS 241. Consumes exactly two MT words per draw.
    #[inline]
    pub fn rnorm(&mut self, mean: f64, sd: f64) -> f64 {
        let big = 134_217_728.0; // 2^27
        let u1 = self.runif();
        let u2 = self.runif();
        let p = (big * u1).trunc() + u2;
        mean + sd * qnorm5(p / big)
    }

    /// R's `exp_rand()` — standard exponential via Ahrens–Dieter (1972).
    /// Variable draw count: 1 initial `runif` plus a rejection loop that
    /// consumes 0..k additional `runif`s.
    pub fn exp_rand(&mut self) -> f64 {
        // q[k-1] = sum(log(2)^k / k!)  k=1,..,16; q[15] = 1.0
        const Q: [f64; 16] = [
            0.6931471805599453,
            0.9333736875190459,
            0.9888777961838675,
            0.9984959252914960,
            0.9998292811061389,
            0.9999833164100727,
            0.9999985691438767,
            0.9999998906925558,
            0.9999999924734159,
            0.9999999995283275,
            0.9999999999728814,
            0.9999999999985598,
            0.9999999999999289,
            0.9999999999999968,
            0.9999999999999999,
            1.0000000000000000,
        ];
        let mut a = 0.0;
        let mut u = self.runif();
        while u <= 0.0 || u >= 1.0 {
            u = self.runif();
        }
        // Leading-bit loop: each time doubling u stays ≤ 1, accumulate q[0].
        loop {
            u += u;
            if u > 1.0 {
                break;
            }
            a += Q[0];
        }
        u -= 1.0;
        if u <= Q[0] {
            return a + u;
        }
        // Rejection loop: draw uniforms, track the running minimum, until
        // u <= q[i].
        let mut i = 0usize;
        let mut ustar = self.runif();
        let mut umin = ustar;
        loop {
            // i is the index *before* increment; we compare u > q[i], then i++.
            if !(u > Q[i]) {
                return a + umin * Q[0];
            }
            ustar = self.runif();
            if umin > ustar {
                umin = ustar;
            }
            i += 1;
        }
    }

    /// `rexp(scale)` — exponential with mean `scale`. Consumes `exp_rand`'s
    /// variable stream.
    #[inline]
    pub fn rexp(&mut self, scale: f64) -> f64 {
        scale * self.exp_rand()
    }

    /// R's `rgamma(shape, scale)` — Ahrens–Dieter GS (shape<1) / GD (shape≥1).
    /// Faithful port of `nmath/rgamma.c`. Note: shape is α (the "a" parameter),
    /// `scale` = 1/rate; mean = shape·scale.
    pub fn rgamma(&mut self, shape: f64, scale: f64) -> f64 {
        // constants
        const SQRT32: f64 = 5.656854;
        const EXP_M1: f64 = 0.36787944117144232159;
        const Q1: f64 = 0.04166669;
        const Q2: f64 = 0.02083148;
        const Q3: f64 = 0.00801191;
        const Q4: f64 = 0.00144121;
        const Q5: f64 = -7.388e-5;
        const Q6: f64 = 2.4511e-4;
        const Q7: f64 = 2.424e-4;
        const A1: f64 = 0.3333333;
        const A2: f64 = -0.250003;
        const A3: f64 = 0.2000062;
        const A4: f64 = -0.1662921;
        const A5: f64 = 0.1423657;
        const A6: f64 = -0.1367177;
        const A7: f64 = 0.1233795;

        let a = shape;
        if a <= 0.0 || scale <= 0.0 {
            if a == 0.0 || scale == 0.0 {
                return 0.0;
            }
            return f64::NAN;
        }
        if !a.is_finite() || !scale.is_finite() {
            return f64::INFINITY;
        }

        if a < 1.0 {
            // GS algorithm
            let e = 1.0 + EXP_M1 * a;
            loop {
                let p = e * self.runif();
                if p >= 1.0 {
                    let x = -((e - p) / a).ln();
                    if self.exp_rand() >= (1.0 - a) * x.ln() {
                        return scale * x;
                    }
                } else {
                    let x = (p.ln() / a).exp();
                    if self.exp_rand() >= x {
                        return scale * x;
                    }
                }
            }
        }

        // GD algorithm (a >= 1)
        // Step 1: per-shape constants
        let s2 = a - 0.5;
        let s = s2.sqrt();
        let d = SQRT32 - s * 12.0;
        // Step 4: per-shape coefficients (recomputed when a changes; static in C,
        // but since we recompute every call here it's just the same formulas).
        let r = 1.0 / a;
        let q0 = ((((((Q7 * r + Q6) * r + Q5) * r + Q4) * r + Q3) * r + Q2) * r + Q1) * r;
        let (b, si, c) = if a <= 3.686 {
            (0.463 + s + 0.178 * s2, 1.235, 0.195 / s - 0.079 + 0.16 * s)
        } else if a <= 13.022 {
            (1.654 + 0.0076 * s2, 1.68 / s + 0.275, 0.062 / s + 0.024)
        } else {
            (1.77, 0.75, 0.1515 / s)
        };

        // Step 2: normal deviate, immediate acceptance
        let t = self.rnorm(0.0, 1.0);
        let x = s + 0.5 * t;
        let ret_val = x * x;
        if t >= 0.0 {
            return scale * ret_val;
        }
        // Step 3: squeeze acceptance
        let u = self.runif();
        if d * u <= t * t * t {
            return scale * ret_val;
        }
        // Step 5–7: quotient test on x
        if x > 0.0 {
            let v = t / (s + s);
            let q = if v.abs() <= 0.25 {
                q0 + 0.5 * t * t
                    * ((((((A7 * v + A6) * v + A5) * v + A4) * v + A3) * v + A2) * v + A1) * v
            } else {
                q0 - s * t + 0.25 * t * t + (s2 + s2) * (1.0 + v).ln()
            };
            if (1.0 - u).ln() <= q {
                return scale * ret_val;
            }
        }
        // Step 8–11: rejection loop
        loop {
            let e = self.exp_rand();
            let u2 = self.runif();
            let uu = u2 + u2 - 1.0; // ∈ [-1, 1]
            let t2 = if uu < 0.0 { b - si * e } else { b + si * e };
            if t2 >= -0.71874483771719 {
                let v = t2 / (s + s);
                let q = if v.abs() <= 0.25 {
                    q0 + 0.5 * t2 * t2
                        * ((((((A7 * v + A6) * v + A5) * v + A4) * v + A3) * v + A2) * v + A1) * v
                } else {
                    q0 - s * t2 + 0.25 * t2 * t2 + (s2 + s2) * (1.0 + v).ln()
                };
                if q > 0.0 {
                    let w = q.exp_m1();
                    if c * uu.abs() <= w * (e - 0.5 * t2 * t2).exp() {
                        let x2 = s + 0.5 * t2;
                        return scale * x2 * x2;
                    }
                }
            }
        }
    }

    /// R's `rbits(bits)`: assemble `ceil(bits/16)` uniform draws into a
    /// `bits`-bit random integer (used by the Rejection sample-kind).
    #[inline]
    pub fn rbits(&mut self, bits: i32) -> f64 {
        let mut v: i64 = 0;
        let mut n = 0i32;
        while n <= bits {
            let v1 = (self.runif() * 65536.0).floor() as i64;
            v = 65536 * v + v1;
            n += 16;
        }
        let mask = if bits >= 63 { -1i64 } else { (1i64 << bits) - 1 };
        (v & mask) as f64
    }

    /// R's `R_unif_index(dn)` under the **Rejection** sample-kind: a uniform
    /// integer index in `[0, dn)` obtained by rejection sampling from the
    /// next power of two.
    pub fn unif_index(&mut self, dn: f64) -> f64 {
        if dn <= 0.0 {
            return 0.0;
        }
        let bits = dn.log2().ceil() as i32;
        loop {
            let dv = self.rbits(bits);
            if !(dn <= dv) {
                return dv;
            }
        }
    }

    /// `sample.int(n, 1)` for n ≥ 1 under Rejection sample-kind: returns an
    /// integer in `1..=n`. Consumes one `R_unif_index(n)`.
    #[inline]
    pub fn sample_int_one(&mut self, n: usize) -> usize {
        // mirrors do_sample: iy = (int)(R_unif_index(n) + 1)
        let idx = self.unif_index(n as f64) as usize;
        idx + 1
    }

    /// `sample(c(...), 1)` — equivalent to `x[sample.int(length(x), 1)]`.
    /// For length(x) == 1, R's `ifelse(length(...)==1, x, sample(x,1))` skips
    /// sampling entirely; mirror that by only consuming RNG when `xs.len() > 1`.
    pub fn sample_one<'a, T: Clone>(&mut self, xs: &'a [T]) -> &'a T {
        if xs.len() == 1 {
            &xs[0]
        } else {
            let idx = self.sample_int_one(xs.len()) - 1;
            &xs[idx]
        }
    }

    /// `runif(min, max)` from Rmath: `min + (max-min) * unif_rand()`.
    #[inline]
    pub fn runif_range(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.runif()
    }

    // ── truncnorm::rtruncnorm port ──────────────────────────────────────

    /// `truncnorm::rtruncnorm(1, a, b, mean, sd)` — single draw from a
    /// truncated normal on `[a, b]`. Faithful port of truncnorm 1.0-9's
    /// `rtruncnorm.c` (Robert 1995 rejection). `a` of `-inf` and `b` of `+inf`
    /// encode one- or two-sided truncation as in R.
    pub fn rtruncnorm(&mut self, a: f64, b: f64, mean: f64, sd: f64) -> f64 {
        // match R's arrival: finite bounds choose algorithm by standardized cuts
        if a.is_finite() && b.is_finite() {
            self.r_truncnorm_both(a, b, mean, sd)
        } else if a == f64::NEG_INFINITY && b.is_finite() {
            // right truncation only
            let beta = (b - mean) / sd;
            mean - sd * self.r_lefttruncnorm(-beta)
        } else if a.is_finite() && b == f64::INFINITY {
            self.r_lefttruncnorm_scaled(a, mean, sd)
        } else {
            // no truncation
            self.rnorm(mean, sd)
        }
    }

    fn r_lefttruncnorm(&mut self, a: f64) -> f64 {
        // standardized version (mean=0, sd=1)
        const T4: f64 = 0.45;
        if a < T4 {
            self.nrs_a_inf(a)
        } else {
            self.ers_a_inf(a)
        }
    }

    fn r_lefttruncnorm_scaled(&mut self, a: f64, mean: f64, sd: f64) -> f64 {
        let alpha = (a - mean) / sd;
        mean + sd * self.r_lefttruncnorm(alpha)
    }

    // exponential rejection sampling (a, inf)
    fn ers_a_inf(&mut self, a: f64) -> f64 {
        let ainv = 1.0 / a;
        loop {
            let x = self.rexp(ainv) + a;
            let rho = (-0.5 * (x - a).powi(2)).exp();
            if self.runif() <= rho {
                return x;
            }
        }
    }

    // normal rejection sampling (a, inf)
    fn nrs_a_inf(&mut self, a: f64) -> f64 {
        loop {
            let x = self.rnorm(0.0, 1.0);
            if x >= a {
                return x;
            }
        }
    }

    // exponential rejection sampling (a, b)
    fn ers_a_b(&mut self, a: f64, b: f64) -> f64 {
        let ainv = 1.0 / a;
        loop {
            let x = self.rexp(ainv) + a;
            let rho = (-0.5 * (x - a).powi(2)).exp();
            if self.runif() <= rho && x <= b {
                return x;
            }
        }
    }

    // normal rejection sampling (a, b)
    fn nrs_a_b(&mut self, a: f64, b: f64) -> f64 {
        loop {
            let x = self.rnorm(0.0, 1.0);
            if x >= a && x <= b {
                return x;
            }
        }
    }

    // half-normal rejection sampling (a, b)
    fn hnrs_a_b(&mut self, a: f64, b: f64) -> f64 {
        loop {
            let x = self.rnorm(0.0, 1.0).abs();
            if x >= a && x <= b {
                return x;
            }
        }
    }

    // uniform rejection sampling (a, b)
    fn urs_a_b(&mut self, a: f64, b: f64) -> f64 {
        let phi_a = dnorm(a, 0.0, 1.0);
        let ub = if a < 0.0 && b > 0.0 {
            M_1_SQRT_2PI
        } else {
            phi_a
        };
        loop {
            let x = self.runif_range(a, b);
            if self.runif() * ub <= dnorm(x, 0.0, 1.0) {
                return x;
            }
        }
    }

    fn r_truncnorm_both(&mut self, a: f64, b: f64, mean: f64, sd: f64) -> f64 {
        const T1: f64 = 0.15;
        const T2: f64 = 2.18;
        const T3: f64 = 0.725;
        let alpha = (a - mean) / sd;
        let beta = (b - mean) / sd;
        let phi_a = dnorm(alpha, 0.0, 1.0);
        let phi_b = dnorm(beta, 0.0, 1.0);
        if beta <= alpha {
            f64::NAN
        } else if alpha <= 0.0 && 0.0 <= beta {
            if phi_a <= T1 || phi_b <= T1 {
                mean + sd * self.nrs_a_b(alpha, beta)
            } else {
                mean + sd * self.urs_a_b(alpha, beta)
            }
        } else if alpha > 0.0 {
            if phi_a / phi_b <= T2 {
                mean + sd * self.urs_a_b(alpha, beta)
            } else if alpha < T3 {
                mean + sd * self.hnrs_a_b(alpha, beta)
            } else {
                mean + sd * self.ers_a_b(alpha, beta)
            }
        } else {
            // alpha < 0, beta <= 0 (mirror)
            if phi_b / phi_a <= T2 {
                mean - sd * self.urs_a_b(-beta, -alpha)
            } else if beta > -T3 {
                mean - sd * self.hnrs_a_b(-beta, -alpha)
            } else {
                mean - sd * self.ers_a_b(-beta, -alpha)
            }
        }
    }
}

// constants Rmath exposes
const M_1_SQRT_2PI: f64 = 0.39894228040143267793994605993438136841236754353;

/// Rmath `dnorm(x, mean, sd)` (log=FALSE).
fn dnorm(x: f64, mean: f64, sd: f64) -> f64 {
    let z = (x - mean) / sd;
    (M_1_SQRT_2PI / sd) * (-0.5 * z * z).exp()
}

/// Wichura's AS 241 normal quantile, transcribed from R's `qnorm.c`
/// (`qnorm5`), for `lower.tail = TRUE`, `log.p = FALSE`.
pub fn qnorm5(p: f64) -> f64 {
    debug_assert!(p > 0.0 && p < 1.0);
    let p_ = p;
    let q = p_ - 0.5;
    if q.abs() <= 0.425 {
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
        let lp = if q > 0.0 { (1.0 - p_).ln() } else { p_.ln() };
        let r = (-lp).sqrt();
        let val = if r <= 5.0 {
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

    /// R: set.seed(5); rexp(3) — C-level `rexp(scale=1)` = Ahrens–Dieter exp_rand.
    /// (R-level `rexp(rate=1)` calls C `rexp(scale=1/rate)`.)
    #[test]
    fn matches_r_rexp() {
        let mut rng = Rng::new(5);
        let got: [f64; 3] = [rng.rexp(1.0), rng.rexp(1.0), rng.rexp(1.0)];
        // Verified against R 4.5.3: set.seed(5); rexp(3)
        let expected = [1.9880099817270283, 0.3704371913336217, 0.0725379411228920];
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-7, "got {g:0.10} expected {e:0.10}");
        }
    }

    /// R: set.seed(5); rexp(3, rate=2.5) — R-level rate → C-level scale = 1/rate.
    /// My `rexp(scale=1/2.5)` should match R's `rexp(rate=2.5)`.
    #[test]
    fn matches_r_rexp_scaled() {
        let mut rng = Rng::new(5);
        let got: [f64; 3] = [rng.rexp(0.4), rng.rexp(0.4), rng.rexp(0.4)];
        // Verified against R: set.seed(5); rexp(3, 2.5)
        let expected = [0.7952039926908113, 0.1481748765334487, 0.0290151764491568];
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-7, "got {g:0.10} expected {e:0.10}");
        }
    }

    /// R: set.seed(42); rgamma(5, shape=2, rate=0.5) — scale = 1/rate = 2.
    #[test]
    fn matches_r_rgamma_shape_ge_1() {
        let mut rng = Rng::new(42);
        let got: Vec<f64> = (0..5).map(|_| rng.rgamma(2.0, 2.0)).collect();
        // Verified against R: set.seed(42); rgamma(5, shape=2, rate=0.5)
        let expected = [7.2979122, 1.7762196, 3.1184396, 0.904365, 2.7456803];
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-6, "got {g:0.10} expected {e:0.10}");
        }
    }

    /// R: set.seed(42); rgamma(5, shape=0.5, rate=1) — scale = 1.
    #[test]
    fn matches_r_rgamma_shape_lt_1() {
        let mut rng = Rng::new(42);
        let got: Vec<f64> = (0..5).map(|_| rng.rgamma(0.5, 1.0)).collect();
        let expected = [0.7605168, 0.2936975, 1.8663435, 1.4817393, 0.2134231];
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-6, "got {g:0.10} expected {e:0.10}");
        }
    }

    /// R: set.seed(99); sapply(1:5, function(i) sample.int(5,1)) → 1 4 5 3 2
    #[test]
    fn matches_r_sample_int_one() {
        let mut rng = Rng::new(99);
        let got: Vec<usize> = (0..5).map(|_| rng.sample_int_one(5)).collect();
        let expected = [1usize, 4, 5, 3, 2];
        for (g, e) in got.iter().zip(expected.iter()) {
            assert_eq!(*g, *e, "sample mismatch");
        }
    }

    /// R: set.seed(7); truncnorm::rtruncnorm(5, a=-1, b=2, mean=0, sd=1)
    #[test]
    fn matches_r_rtruncnorm_two_sided() {
        let mut rng = Rng::new(7);
        let got: Vec<f64> = (0..5)
            .map(|_| rng.rtruncnorm(-1.0, 2.0, 0.0, 1.0))
            .collect();
        let expected = [-0.6942925, -0.412293, -0.9706733, -0.9472799, 0.7481393];
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-6, "got {g:0.10} expected {e:0.10}");
        }
    }

    /// R: set.seed(7); truncnorm::rtruncnorm(5, a=0.5, b=Inf, mean=0, sd=1)
    #[test]
    fn matches_r_rtruncnorm_left() {
        let mut rng = Rng::new(7);
        let got: Vec<f64> = (0..5)
            .map(|_| rng.rtruncnorm(0.5, f64::INFINITY, 0.0, 1.0))
            .collect();
        let expected = [0.5966922, 1.6680417, 0.7299245, 1.5912478, 2.0037145];
        for (g, e) in got.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-6, "got {g:0.10} expected {e:0.10}");
        }
    }
}
