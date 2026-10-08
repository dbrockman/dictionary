//! `dictdb`: the app's own dictionary storage format.
//!
//! Importers convert source dictionaries into a `.dictdb` directory with
//! [`DictWriter`]; the app opens them with [`Dictionary`], which memory-maps
//! every file so opening is constant-time and searching touches only the
//! pages it needs. See [`format`] for the on-disk layout.

mod error;
pub mod format;
mod normalize;
mod reader;
mod writer;

pub use error::{Error, Result};
pub use format::{DictInfo, Meta};
pub use normalize::normalize_key;
pub use reader::{Dictionary, Hit};
pub use writer::{DictWriter, EntrySpec, KeySpec};
