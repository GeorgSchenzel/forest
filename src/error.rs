use std::fmt;

/// `Fail` exits 1, `Refused` (a safety check said no) exits 2.
#[derive(Debug)]
pub enum Error {
    Fail(String),
    Refused(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Fail(_) => 1,
            Error::Refused(_) => 2,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Error::Fail(_) => "error",
            Error::Refused(_) => "refused",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Fail(m) | Error::Refused(m) => f.write_str(m),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Fail(e.to_string())
    }
}

macro_rules! fail {
    ($($arg:tt)*) => { $crate::error::Error::Fail(format!($($arg)*)) };
}
pub(crate) use fail;
