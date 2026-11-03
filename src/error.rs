use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Bad input from the caller, such as pixels that don't fill the image.
    Invalid(String),
    /// The PNG is damaged, cut short or not a PNG.
    Corrupt(String),
    /// Decoding would produce more than the caller allowed.
    TooLarge { limit: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }

    pub(crate) fn corrupt(msg: impl Into<String>) -> Self {
        Error::Corrupt(msg.into())
    }
}

impl From<black_zlib::Error> for Error {
    fn from(e: black_zlib::Error) -> Self {
        match e {
            black_zlib::Error::Corrupt(msg) => Error::Corrupt(format!("the compressed pixels: {msg}")),
            black_zlib::Error::TooLarge { limit } => Error::TooLarge { limit },
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(msg) => f.write_str(msg),
            Error::Corrupt(msg) => write!(f, "corrupt PNG: {msg}"),
            Error::TooLarge { limit } => write!(f, "the image is bigger than the {limit}-byte limit"),
        }
    }
}

impl std::error::Error for Error {}
