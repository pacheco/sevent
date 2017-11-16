use std;
use bincode;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Bincode(bincode::Error),
    InvalidId,
}

impl From<std::io::Error> for Error {
    fn from(other: std::io::Error) -> Self {
        Error::Io(other)
    }
}

impl From<bincode::Error> for Error {
    fn from(other: bincode::Error) -> Self {
        Error::Bincode(other)
    }
}
