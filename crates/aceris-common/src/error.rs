//! Error type shared across the emulator.

use std::fmt;

/// Convenient result alias used throughout Aceris.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur while loading or running a game.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    UnsupportedFormat,
    InvalidNcsdHeader(&'static str),
    InvalidNcsdPartition { index: usize, reason: &'static str },
    OverlappingNcsdPartitions { first: usize, second: usize },
    NoImageLoaded,
    InvalidAccess { addr: u32 },
    UndefinedInstruction { pc: u32, word: u32 },
    Unimplemented(&'static str),
    SelfTest(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(source) => write!(formatter, "I/O error: {source}"),
            Self::UnsupportedFormat => {
                write!(formatter, "unrecognised or unsupported game image format")
            }
            Self::InvalidNcsdHeader(reason) => {
                write!(formatter, "invalid NCSD header: {reason}")
            }
            Self::InvalidNcsdPartition { index, reason } => {
                write!(formatter, "invalid NCSD partition {index}: {reason}")
            }
            Self::OverlappingNcsdPartitions { first, second } => {
                write!(formatter, "NCSD partitions {first} and {second} overlap")
            }
            Self::NoImageLoaded => write!(formatter, "no game image has been loaded"),
            Self::InvalidAccess { addr } => {
                write!(formatter, "invalid memory access at {addr:#010x}")
            }
            Self::UndefinedInstruction { pc, word } => {
                write!(
                    formatter,
                    "undefined instruction {word:#010x} at {pc:#010x}"
                )
            }
            Self::Unimplemented(feature) => write!(formatter, "not implemented yet: {feature}"),
            Self::SelfTest(reason) => write!(formatter, "CPU self-test failed: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Self {
        Self::Io(source)
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn displays_every_non_io_error_variant() {
        let cases = [
            (
                Error::UnsupportedFormat,
                "unrecognised or unsupported game image format",
            ),
            (
                Error::InvalidNcsdHeader("bad magic"),
                "invalid NCSD header: bad magic",
            ),
            (
                Error::InvalidNcsdPartition {
                    index: 2,
                    reason: "outside image",
                },
                "invalid NCSD partition 2: outside image",
            ),
            (
                Error::OverlappingNcsdPartitions {
                    first: 1,
                    second: 3,
                },
                "NCSD partitions 1 and 3 overlap",
            ),
            (Error::NoImageLoaded, "no game image has been loaded"),
            (
                Error::InvalidAccess { addr: 0x12 },
                "invalid memory access at 0x00000012",
            ),
            (
                Error::UndefinedInstruction {
                    pc: 0x1000,
                    word: 0xFFFF_FFFF,
                },
                "undefined instruction 0xffffffff at 0x00001000",
            ),
            (
                Error::Unimplemented("emulation loop"),
                "not implemented yet: emulation loop",
            ),
            (
                Error::SelfTest("bad result"),
                "CPU self-test failed: bad result",
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn converts_io_errors_and_preserves_the_source() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let error = Error::from(io_error);

        assert_eq!(error.to_string(), "I/O error: missing file");
        assert!(std::error::Error::source(&error).is_some());
    }
}
