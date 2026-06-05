//! An idiomatic Rust implementation of the **Senbay** format: compact text that
//! packs sensor data for embedding as QR codes in video.
//!
//! The API is organized around Rust idioms:
//!
//! - [`Value`] is a typed enum instead of stringly-typed map values.
//! - [`Record`] is an ordered field collection with a builder-style API and a
//!   deterministic encoding order.
//! - [`Senbay`] is the codec; [`Encoding`] selects the plain or compressed form.
//! - Fallible operations return [`Result`] with a structured [`Error`]; encoding
//!   an in-memory record is infallible.
//!
//! ```
//! use senbay_rs::{Encoding, Record, Senbay};
//!
//! let codec = Senbay::new();
//!
//! let mut record = Record::new();
//! record
//!     .set("TIME", 1_700_000_000_000_i64)
//!     .set("LATI", 35.6895)
//!     .set("MEMO", "hello");
//!
//! let text = codec.encode(&record, Encoding::Compressed);
//! let decoded = codec.decode(&text);
//!
//! assert_eq!(decoded.get("LATI").unwrap().as_f64(), Some(35.6895));
//! assert_eq!(decoded.get("MEMO").unwrap().as_str(), Some("hello"));
//! ```
//!
//! The QR/OpenCV [`Reader`] and [`Writer`] live behind the `video` feature.

mod codec;
mod error;
mod radix;
mod record;
mod value;

pub use codec::Senbay;
pub use error::{Error, Result};
pub use radix::Radix;
pub use record::{Encoding, Iter, Record};
pub use value::Value;

#[cfg(feature = "video")]
mod reader;
#[cfg(feature = "video")]
mod video_backend;
#[cfg(feature = "video")]
mod writer;

#[cfg(feature = "video")]
pub use reader::Reader;
#[cfg(feature = "video")]
pub use writer::Writer;

/// Key code for the Escape key, used to stop the interactive video windows.
#[cfg(feature = "video")]
pub(crate) const KEY_CODE_ESC: i32 = 27;
