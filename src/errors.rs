use std;
use bincode;
use mio_more;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Bincode(bincode::Error),
    Timer(mio_more::timer::TimerError),
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

impl From<mio_more::timer::TimerError> for Error {
    fn from(other: mio_more::timer::TimerError) -> Self {
        Error::Timer(other)
    }
}
