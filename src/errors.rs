use std;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    InvalidId,
}

impl From<std::io::Error> for Error {
    fn from(other: std::io::Error) -> Self {
        Error::Io(other)
    }
}
