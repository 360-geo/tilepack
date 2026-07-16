//! Error types for parsing and writing tilepack containers.

use thiserror::Error;

/// Failure parsing a tilepack header, descriptor table, index, or body.
///
/// [`ParseError::Truncated`] is not a corruption error: it reports how many
/// bytes a reader must have in hand to make progress. A range-reading client
/// fetches at least `needed` bytes and retries. Each retry returns a
/// monotonically larger `needed` until the full front matter is present.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The buffer is shorter than the next parse step requires. `needed` is
    /// the total prefix length (from offset 0) that would let parsing advance.
    #[error("truncated: need {needed} bytes from the start of the file")]
    Truncated { needed: usize },

    /// The first four bytes are not the tilepack magic `TPCK`.
    #[error("bad magic: not a tilepack file")]
    BadMagic,

    /// The version byte names a layout this reader does not implement.
    #[error("unsupported version {found} (this reader speaks {supported})")]
    BadVersion { found: u8, supported: u8 },

    /// `face_count` was neither 1 (planar) nor 6 (cubemap).
    #[error("invalid face_count {0}: must be 1 or 6")]
    BadFaceCount(u8),

    /// A structural field violated a minimum or a cross-field invariant.
    #[error("inconsistent header: {0}")]
    Inconsistent(&'static str),

    /// The offset index was not non-decreasing, or an offset pointed outside
    /// the file.
    #[error("bad tile index: {0}")]
    BadIndex(&'static str),
}

/// Failure assembling a tilepack container.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// A header or descriptor field was out of range for the wire format.
    #[error("invalid parameter: {0}")]
    InvalidParams(&'static str),

    /// A blob was addressed by a tile location or ordinal outside the layout.
    #[error("tile out of range: {0}")]
    TileOutOfRange(&'static str),
}
