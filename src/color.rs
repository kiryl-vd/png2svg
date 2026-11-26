//! Color math in the perceptual Oklab color space.
//!
//! The SVG output must be byte-identical on every platform (requirement
//! SVG-07). The standard library functions `powf` and `cbrt` call the math
//! library of the operating system, and these may give results that differ in
//! the last bit. For this reason this module does not use them. It computes
//! roots with Newton iterations that use only `+ - * /`, which are exactly
//! defined by IEEE 754 on all platforms.

/// A color in Oklab: lightness `l` (0..1), and the opponent axes `a`, `b`.
pub type Lab = [f64; 3];

/// Cube root for `x >= 0`, using only basic arithmetic.
pub fn cbrt(x: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() {
        return 0.0;
    }
    // A first guess from the bit pattern: dividing the exponent field by 3
    // gives a number that is within about 10 percent of the real root.
    let guess_bits = x.to_bits() / 3 + 0x2AA0_0000_0000_0000;
    let mut r = f64::from_bits(guess_bits);
    // Newton iteration for r^3 = x. Quadratic convergence: 6 steps are
    // enough to reach full double precision from a 10 percent error.
    for _ in 0..6 {
        r = (2.0 * r + x / (r * r)) / 3.0;
    }
    r
}

/// Fifth root for `x >= 0`, using only basic arithmetic.
fn root5(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    // Start at 1 (the input is at most about 1) and iterate many times. This is
    // only used to build a table of 256 numbers, so the cost does not matter.
    let mut r = 1.0;
    for _ in 0..200 {
        let r2 = r * r;
        r = (4.0 * r + x / (r2 * r2)) / 5.0;
    }
    r
}

/// Converts an sRGB channel value (0..255) to linear light (0..1).
fn srgb_to_linear(value: u8) -> f64 {
    let c = f64::from(value) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        // ((c + 0.055) / 1.055) ^ 2.4  =  y^2 * (y^2)^(1/5)
        let y = (c + 0.055) / 1.055;
        let y2 = y * y;
        y2 * root5(y2)
    }
}

/// Table that maps every 8-bit sRGB value to linear light.
pub struct LinearTable([f64; 256]);

impl LinearTable {
    /// Builds the table.
    pub fn new() -> LinearTable {
        let mut table = [0.0; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            *slot = srgb_to_linear(i as u8);
        }
        LinearTable(table)
    }

    /// Converts an sRGB color to Oklab.
    pub fn oklab(&self, rgb: [u8; 3]) -> Lab {
        let r = self.0[usize::from(rgb[0])];
        let g = self.0[usize::from(rgb[1])];
        let b = self.0[usize::from(rgb[2])];
        // Matrix from linear sRGB to the LMS cone response (Björn Ottosson).
        let l = cbrt(0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b);
        let m = cbrt(0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b);
        let s = cbrt(0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b);
        [
            0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
            1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
            0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
        ]
    }
}

impl Default for LinearTable {
    fn default() -> Self {
        LinearTable::new()
    }
}

/// Squared distance between two Oklab colors.
pub fn distance_sq(a: &Lab, b: &Lab) -> f64 {
    let dl = a[0] - b[0];
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    dl * dl + da * da + db * db
}

/// Small deterministic random number generator (xorshift64*).
///
/// A fixed seed gives the same sequence on every platform, so the result of
/// the conversion does not change between runs (requirement ALG-14).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Creates a generator. A zero seed is replaced because xorshift cannot
    /// leave the all-zero state.
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A random number in `[0, 1)` with 53 bits of precision.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbrt_is_accurate() {
        for &x in &[1e-12, 1e-6, 0.001, 0.125, 0.5, 1.0, 8.0, 27.0, 1000.0] {
            let r = cbrt(x);
            assert!((r * r * r - x).abs() <= x * 1e-14, "cbrt({x}) = {r}");
        }
        assert_eq!(cbrt(0.0), 0.0);
        assert_eq!(cbrt(-1.0), 0.0);
    }

    #[test]
    fn linear_table_endpoints_and_middle() {
        let t = LinearTable::new();
        assert_eq!(t.0[0], 0.0);
        assert!((t.0[255] - 1.0).abs() < 1e-12);
        // sRGB 128 is about 0.2158 in linear light.
        assert!((t.0[128] - 0.215_860_5).abs() < 1e-6);
    }

    #[test]
    fn oklab_reference_values() {
        let t = LinearTable::new();
        let white = t.oklab([255, 255, 255]);
        assert!((white[0] - 1.0).abs() < 1e-3 && white[1].abs() < 1e-3 && white[2].abs() < 1e-3);
        let black = t.oklab([0, 0, 0]);
        assert!(black[0].abs() < 1e-9);
        // Pure red in Oklab is about (0.628, 0.225, 0.126).
        let red = t.oklab([255, 0, 0]);
        assert!((red[0] - 0.628).abs() < 2e-3, "{red:?}");
        assert!((red[1] - 0.2249).abs() < 2e-3, "{red:?}");
        assert!((red[2] - 0.1258).abs() < 2e-3, "{red:?}");
    }

    #[test]
    fn distance_is_symmetric_and_zero_for_equal_colors() {
        let t = LinearTable::new();
        let a = t.oklab([10, 200, 30]);
        let b = t.oklab([200, 10, 30]);
        assert_eq!(distance_sq(&a, &a), 0.0);
        assert_eq!(distance_sq(&a, &b), distance_sq(&b, &a));
        assert!(distance_sq(&a, &b) > 0.0);
    }

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            let x = a.next_f64();
            assert_eq!(x, b.next_f64());
            assert!((0.0..1.0).contains(&x));
        }
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
        assert_ne!(Rng::new(0).next_u64(), 0);
    }
}
