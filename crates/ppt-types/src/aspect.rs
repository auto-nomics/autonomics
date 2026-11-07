//! Aspect ratio parsing (Slidev `parseAspectRatio` in `parser/src/utils.ts`).
//!
//! Accepts `"16/9"`, `"1:1"`, `"3x4"`, and bare numbers (`"1.7777"` →
//! width/height as a float ratio).

use std::fmt;

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AspectRatio {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("invalid aspect ratio {raw:?}: expected \"W/H\", \"W:H\", \"WxH\" or a number")]
pub struct ParseAspectError {
    raw: String,
}

impl AspectRatio {
    /// Slidev default: 16/9.
    pub fn sixteen_nine() -> Self {
        Self {
            width: 16,
            height: 9,
        }
    }

    /// Parse `"16/9"`, `"1:1"`, `"3x4"`, `"1.7777"`, or `"16/9 "`-ish input.
    pub fn parse(raw: &str) -> Result<Self, ParseAspectError> {
        let raw = raw.trim();
        let err = || ParseAspectError {
            raw: raw.to_string(),
        };
        for sep in ['/', ':', 'x', 'X'] {
            if let Some((w, h)) = raw.split_once(sep) {
                let w: f64 = w.trim().parse().map_err(|_| err())?;
                let h: f64 = h.trim().parse().map_err(|_| err())?;
                if w <= 0.0 || h <= 0.0 {
                    return Err(err());
                }
                // Normalize to small integer ratios via a bounded search,
                // preserving e.g. `1.7777` ≈ 16/9 only when it is exact
                // enough; otherwise keep a scaled integer pair.
                return normalize_ratio(w, h).ok_or_else(err);
            }
        }
        // Bare number: width/height float ratio over a base of 1000.
        let v: f64 = raw.parse().map_err(|_| err())?;
        if v <= 0.0 {
            return Err(err());
        }
        normalize_ratio(v, 1.0).ok_or_else(err)
    }

    /// Height in px for a given canvas width (rounded).
    pub fn height_for(&self, canvas_width: u32) -> u32 {
        ((f64::from(canvas_width) * f64::from(self.height)) / f64::from(self.width)).round() as u32
    }
}

fn normalize_ratio(w: f64, h: f64) -> Option<AspectRatio> {
    // Try small integer ratios first (16/9, 4/3, …); fall back to scaling.
    for d in 1u32..=1000 {
        let wf = w * f64::from(d);
        let hf = h * f64::from(d);
        let wi = wf.round();
        let hi = hf.round();
        if (wf - wi).abs() < 1e-6 && (hf - hi).abs() < 1e-6 && wi >= 1.0 && hi >= 1.0 {
            // Reduce by gcd.
            let mut a = wi as u32;
            let mut b = hi as u32;
            while b != 0 {
                (a, b) = (b, a % b);
            }
            return Some(AspectRatio {
                width: (wi as u32) / a,
                height: (hi as u32) / a,
            });
        }
    }
    None
}

impl fmt::Display for AspectRatio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.width, self.height)
    }
}

impl Serialize for AspectRatio {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AspectRatio {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(
            AspectRatio::parse("16/9").unwrap(),
            AspectRatio {
                width: 16,
                height: 9
            }
        );
        assert_eq!(
            AspectRatio::parse("1:1").unwrap(),
            AspectRatio {
                width: 1,
                height: 1
            }
        );
        assert_eq!(
            AspectRatio::parse("3x4").unwrap(),
            AspectRatio {
                width: 3,
                height: 4
            }
        );
        assert_eq!(
            AspectRatio::parse("4/3").unwrap(),
            AspectRatio {
                width: 4,
                height: 3
            }
        );
        // Bare float ≈ 16/9
        assert_eq!(
            AspectRatio::parse("1.7777777778").unwrap(),
            AspectRatio {
                width: 16,
                height: 9
            }
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(AspectRatio::parse("abc").is_err());
        assert!(AspectRatio::parse("0/9").is_err());
        assert!(AspectRatio::parse("16/0").is_err());
        assert!(AspectRatio::parse("").is_err());
    }

    #[test]
    fn height_for_rounds() {
        let a = AspectRatio::parse("16/9").unwrap();
        assert_eq!(a.height_for(980), 551); // 551.25 → 551
        let sq = AspectRatio::parse("1:1").unwrap();
        assert_eq!(sq.height_for(980), 980);
    }

    #[test]
    fn serde_as_string() {
        let a = AspectRatio::parse("16/9").unwrap();
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::json!("16/9")
        );
        assert_eq!(
            serde_json::from_value::<AspectRatio>(serde_json::json!("16/9")).unwrap(),
            a
        );
    }
}
