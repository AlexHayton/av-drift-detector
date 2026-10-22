//! The audio side of the test pattern: a 16-bit frame id carried by tones,
//! with music under them.

pub mod code;
pub mod detect;
pub mod music;
pub mod synth;

pub use self::code::ToneCode;
pub use self::detect::{AudioCell, CellDetector};
pub use self::music::Music;
pub use self::synth::{ToneSynth, synthesize};
