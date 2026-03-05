use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum AppError {
    Arg(String),
    Io {
        path: Option<PathBuf>,
        source: io::Error,
    },
    Decode(String),
    Encode(String),
    QualityTooLow {
        minimum: u8,
        actual: u8,
    },
    OutputLarger {
        input_bytes: u64,
        output_bytes: u64,
    },
}

impl AppError {
    pub fn exit_code(&self) -> i32 {
        match self {
            AppError::Arg(_) => 2,
            AppError::Io { .. } => 3,
            AppError::Decode(_) | AppError::Encode(_) => 4,
            AppError::QualityTooLow { .. } => 98,
            AppError::OutputLarger { .. } => 99,
        }
    }

    pub fn io_with_path(path: impl AsRef<Path>, source: io::Error) -> Self {
        Self::Io {
            path: Some(path.as_ref().to_path_buf()),
            source,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Arg(msg) => write!(f, "{msg}"),
            AppError::Io { path, source } => match path {
                Some(path) => write!(f, "io error at {}: {source}", path.display()),
                None => write!(f, "io error: {source}"),
            },
            AppError::Decode(msg) => write!(f, "decode error: {msg}"),
            AppError::Encode(msg) => write!(f, "encode error: {msg}"),
            AppError::QualityTooLow { minimum, actual } => {
                write!(f, "quality too low: actual={actual}, minimum={minimum}")
            }
            AppError::OutputLarger {
                input_bytes,
                output_bytes,
            } => write!(
                f,
                "output would be larger: input={} bytes, output={} bytes",
                input_bytes, output_bytes
            ),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AppError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
