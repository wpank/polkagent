//! Error types for the SCALE codec.

use thiserror::Error;

/// Errors that can occur during SCALE encoding and decoding operations.
#[derive(Debug, Error)]
pub enum CodecError {
    /// The input bytes were exhausted before decoding was complete.
    #[error("unexpected end of input at offset {offset}")]
    UnexpectedEof {
        /// Byte offset where the EOF was encountered.
        offset: usize,
    },

    /// A value was encountered that does not match the expected type.
    #[error("invalid type: {message}")]
    InvalidType {
        /// Description of the type mismatch.
        message: String,
    },

    /// The metadata version is not supported by this codec.
    #[error("unsupported metadata version: {version}")]
    UnsupportedVersion {
        /// The version number that was encountered.
        version: u8,
    },

    /// An error occurred while decoding a value.
    #[error("decode error: {message}")]
    DecodeError {
        /// Description of the decode failure.
        message: String,
    },

    /// An error occurred while encoding a value.
    #[error("encode error: {message}")]
    EncodeError {
        /// Description of the encode failure.
        message: String,
    },
}

impl CodecError {
    /// Construct an [`UnexpectedEof`](Self::UnexpectedEof) error at the given offset.
    pub fn unexpected_eof(offset: usize) -> Self {
        Self::UnexpectedEof { offset }
    }

    /// Construct an [`InvalidType`](Self::InvalidType) error with a message.
    pub fn invalid_type(message: impl Into<String>) -> Self {
        Self::InvalidType {
            message: message.into(),
        }
    }

    /// Construct an [`UnsupportedVersion`](Self::UnsupportedVersion) error.
    pub fn unsupported_version(version: u8) -> Self {
        Self::UnsupportedVersion { version }
    }

    /// Construct a [`DecodeError`](Self::DecodeError) with a message.
    pub fn decode(message: impl Into<String>) -> Self {
        Self::DecodeError {
            message: message.into(),
        }
    }

    /// Construct an [`EncodeError`](Self::EncodeError) with a message.
    pub fn encode(message: impl Into<String>) -> Self {
        Self::EncodeError {
            message: message.into(),
        }
    }
}

/// Convenience alias for a [`Result`] using [`CodecError`].
pub type Result<T> = std::result::Result<T, CodecError>;
