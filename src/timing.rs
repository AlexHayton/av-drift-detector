//! Exact frame-rate arithmetic and time parsing.

use std::fmt;
use std::str::FromStr;

use serde::{Serialize, Serializer};

use crate::{Error, Result};

/// A frame rate stored as an exact reduced fraction, e.g. `30000/1001` for 29.97 fps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameRate {
    num: u32,
    den: u32,
}

/// NTSC-style rates (`n * 1000 / 1001`) recognised when given as decimals like `29.97`.
const NTSC_BASES: [u32; 6] = [24, 30, 48, 60, 120, 240];

impl FrameRate {
    pub fn new(num: u32, den: u32) -> Result<Self> {
        if num == 0 || den == 0 {
            return Err(Error::InvalidFrameRate(format!("{num}/{den}")));
        }
        let g = gcd(num, den);
        let rate = FrameRate {
            num: num / g,
            den: den / g,
        };
        if !(1.0..=480.0).contains(&rate.as_f64()) {
            return Err(Error::InvalidFrameRate(format!(
                "{num}/{den} (must be between 1 and 480 fps)"
            )));
        }
        Ok(rate)
    }

    pub fn num(self) -> u32 {
        self.num
    }

    pub fn den(self) -> u32 {
        self.den
    }

    pub fn as_f64(self) -> f64 {
        f64::from(self.num) / f64::from(self.den)
    }

    /// Duration of one frame in seconds.
    pub fn frame_duration(self) -> f64 {
        f64::from(self.den) / f64::from(self.num)
    }

    /// Presentation time of `frame` in seconds.
    pub fn frame_time(self, frame: i64) -> f64 {
        frame as f64 * self.frame_duration()
    }

    /// The integer rate used for timecode counting (30 for 29.97).
    pub fn nominal(self) -> u32 {
        ((self.num + self.den / 2) / self.den).max(1)
    }

    /// Whether SMPTE drop-frame timecode applies (29.97 and 59.94).
    pub fn is_drop_frame(self) -> bool {
        self.den == 1001 && (self.num == 30000 || self.num == 60000)
    }

    /// Audio samples per frame at `sample_rate` (fractional for NTSC rates).
    pub fn samples_per_frame(self, sample_rate: u32) -> f64 {
        f64::from(sample_rate) * self.frame_duration()
    }

    /// Index of the first audio sample whose time is at or after the start of `frame`.
    pub fn frame_start_sample(self, frame: i64, sample_rate: u32) -> i64 {
        let n = i128::from(frame) * i128::from(sample_rate) * i128::from(self.den);
        let d = i128::from(self.num);
        // Ceiling division that is correct for negative frames too.
        (-(-n).div_euclid(d)) as i64
    }

    /// Number of whole frames needed to cover `seconds` (at least one).
    pub fn frames_in(self, seconds: f64) -> u64 {
        ((seconds * self.as_f64()).round() as u64).max(1)
    }

    /// Short human label, e.g. `29.97` or `30`.
    pub fn label(self) -> String {
        if self.den == 1 {
            return self.num.to_string();
        }
        let s = format!("{:.3}", self.as_f64());
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

impl fmt::Display for FrameRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl FromStr for FrameRate {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let bad = || Error::InvalidFrameRate(s.to_string());
        if let Some((n, d)) = s.split_once('/') {
            let n = n.trim().parse().map_err(|_| bad())?;
            let d = d.trim().parse().map_err(|_| bad())?;
            return FrameRate::new(n, d);
        }
        let v: f64 = s.parse().map_err(|_| bad())?;
        if !v.is_finite() || v <= 0.0 {
            return Err(bad());
        }
        for base in NTSC_BASES {
            let ntsc = f64::from(base) * 1000.0 / 1001.0;
            if (v - ntsc).abs() < 0.006 {
                return FrameRate::new(base * 1000, 1001);
            }
        }
        if (v - v.round()).abs() < 1e-9 {
            return FrameRate::new(v.round() as u32, 1);
        }
        FrameRate::new((v * 1000.0).round() as u32, 1000)
    }
}

impl Serialize for FrameRate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl From<FrameRate> for ffmpeg_next::Rational {
    fn from(rate: FrameRate) -> Self {
        ffmpeg_next::Rational::new(rate.num as i32, rate.den as i32)
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Parses a signed time into seconds.
///
/// Accepts plain seconds (`2.5`), suffixed values (`250ms`, `1.5s`) and clock
/// notation (`01:02.5`, `00:01:02.500`), each optionally prefixed with `+`/`-`.
pub fn parse_time(input: &str) -> Result<f64> {
    let s = input.trim();
    let bad = || Error::InvalidTime(input.to_string());
    let (sign, body) = match s.as_bytes().first() {
        Some(b'-') => (-1.0, &s[1..]),
        Some(b'+') => (1.0, &s[1..]),
        _ => (1.0, s),
    };
    let body = body.trim();
    let number = |t: &str| -> Result<f64> {
        let v: f64 = t.trim().parse().map_err(|_| bad())?;
        if v.is_finite() && v >= 0.0 {
            Ok(v)
        } else {
            Err(bad())
        }
    };
    let seconds = if let Some(ms) = body.strip_suffix("ms") {
        number(ms)? / 1000.0
    } else if let Some(sec) = body.strip_suffix('s') {
        number(sec)?
    } else if body.contains(':') {
        let parts: Vec<&str> = body.split(':').collect();
        if parts.len() > 3 {
            return Err(bad());
        }
        let mut total = 0.0;
        for (i, part) in parts.iter().enumerate() {
            let v = number(part)?;
            if i + 1 < parts.len() && v.fract() != 0.0 {
                return Err(bad());
            }
            total = total * 60.0 + v;
        }
        total
    } else {
        number(body)?
    };
    Ok(sign * seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr(s: &str) -> FrameRate {
        s.parse().unwrap()
    }

    #[test]
    fn parses_frame_rates() {
        assert_eq!(fr("30"), FrameRate::new(30, 1).unwrap());
        assert_eq!(fr("29.97"), FrameRate::new(30000, 1001).unwrap());
        assert_eq!(fr("23.976"), FrameRate::new(24000, 1001).unwrap());
        assert_eq!(fr("23.98"), FrameRate::new(24000, 1001).unwrap());
        assert_eq!(fr("59.94"), FrameRate::new(60000, 1001).unwrap());
        assert_eq!(fr("119.88"), FrameRate::new(120000, 1001).unwrap());
        assert_eq!(fr("60000/1001"), FrameRate::new(60000, 1001).unwrap());
        assert_eq!(fr("50/2"), FrameRate::new(25, 1).unwrap());
        assert_eq!(fr("12.5"), FrameRate::new(25, 2).unwrap());
        assert!("0".parse::<FrameRate>().is_err());
        assert!("abc".parse::<FrameRate>().is_err());
        assert!("1000".parse::<FrameRate>().is_err());
    }

    #[test]
    fn labels_and_display() {
        assert_eq!(fr("29.97").to_string(), "30000/1001");
        assert_eq!(fr("29.97").label(), "29.97");
        assert_eq!(fr("23.976").label(), "23.976");
        assert_eq!(fr("25").label(), "25");
        assert_eq!(fr("29.97").nominal(), 30);
        assert!(fr("59.94").is_drop_frame());
        assert!(!fr("23.976").is_drop_frame());
    }

    #[test]
    fn ntsc_sample_schedule_is_exact() {
        let rate = fr("29.97");
        // 30000 frames at 29.97 fps last exactly 1001 seconds.
        assert_eq!(rate.frame_start_sample(30000, 48000), 1001 * 48000);
        let lens: Vec<i64> = (0..10)
            .map(|n| rate.frame_start_sample(n + 1, 48000) - rate.frame_start_sample(n, 48000))
            .collect();
        assert!(lens.iter().all(|&l| l == 1601 || l == 1602), "{lens:?}");
        assert_eq!(fr("30").frame_start_sample(-1, 48000), -1600);
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("2.5").unwrap(), 2.5);
        assert_eq!(parse_time("250ms").unwrap(), 0.25);
        assert_eq!(parse_time("+78.5ms").unwrap(), 0.0785);
        assert_eq!(parse_time("-40ms").unwrap(), -0.04);
        assert_eq!(parse_time("1.5s").unwrap(), 1.5);
        assert_eq!(parse_time("00:00:05").unwrap(), 5.0);
        assert_eq!(parse_time("01:02.5").unwrap(), 62.5);
        assert_eq!(parse_time("1:00:00").unwrap(), 3600.0);
        assert!(parse_time("1.5:00").is_err());
        assert!(parse_time("ms").is_err());
        assert!(parse_time("nan").is_err());
    }
}
