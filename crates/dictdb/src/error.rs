use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid meta.json: {0}")]
    Meta(#[from] serde_json::Error),
    #[error(transparent)]
    Fst(#[from] fst::Error),
    #[error("{path}: unsupported dictdb format version {version}")]
    UnsupportedVersion { path: PathBuf, version: u32 },
    #[error("{path}: corrupt dictionary ({reason})")]
    Corrupt { path: PathBuf, reason: &'static str },
    #[error("entry {0} does not exist")]
    NoSuchEntry(u32),
    #[error("dictionary exceeds format limits")]
    TooLarge,
}
