//! Error type for the crate.

use std::fmt;

/// A specialized `Result` type for senbay operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur while configuring or running a codec.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The requested positional notation (radix) was out of range.
    InvalidRadix {
        /// The value that was requested.
        value: u32,
        /// The maximum radix supported by the digit table.
        max: u32,
    },
    /// An error bubbled up from OpenCV while reading or writing video.
    #[cfg(feature = "video")]
    Opencv(opencv::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidRadix { value, max } => {
                write!(f, "radix must be in 2..={max}, got {value}")
            }
            #[cfg(feature = "video")]
            Error::Opencv(err) => write!(f, "opencv error: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::InvalidRadix { .. } => None,
            #[cfg(feature = "video")]
            Error::Opencv(err) => Some(err),
        }
    }
}

#[cfg(feature = "video")]
impl From<opencv::Error> for Error {
    fn from(err: opencv::Error) -> Self {
        Error::Opencv(err)
    }
}
