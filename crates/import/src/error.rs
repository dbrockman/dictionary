use std::path::PathBuf;

use crate::simplify::SimplifyError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Db(#[from] dictdb::Error),
    #[error("{path}: {source}")]
    Plist { path: PathBuf, source: plist::Error },
    #[error("{path}: XML error at byte {position}: {source}")]
    Xml {
        path: PathBuf,
        position: u64,
        source: quick_xml::Error,
    },
    #[error(transparent)]
    Entry(#[from] SimplifyError),
    #[error("{path}: {reason}")]
    Unsupported { path: PathBuf, reason: String },
    #[error("{0}: not a dictionary bundle or dictionary source XML")]
    Unrecognized(PathBuf),
}

/// Attaches the path to I/O errors.
pub(crate) trait IoContext<T> {
    fn at(self, path: &std::path::Path) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: &std::path::Path) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })
    }
}
