use std::cell::RefCell;
use std::io::ErrorKind::WouldBlock;
use std::io::Read;
use std::io::Write;
use std::io;

use mio::Ready;
use mio::PollOpt;
use mio::net::TcpStream;

use ::Error;

const READ_SIZE: usize = 8*1024;

pub trait ConnectionHandler {
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>);
    fn on_disconnect(&mut self, id: usize, error: Option<Error>);
}

impl<R, D> ConnectionHandler for ConnectionHandlerClosures<R,D>
    where R: FnMut(usize, &mut Vec<u8>),
          D: FnMut(usize, Option<Error>),
{
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        (self.on_read)(id, buf)
    }
    fn on_disconnect(&mut self, id: usize, error: Option<Error>) {
        (self.on_disconnect)(id, error)
    }
}

pub struct ConnectionHandlerClosures<R,D>
    where R: FnMut(usize, &mut Vec<u8>),
          D: FnMut(usize, Option<Error>),
{
    pub on_read: R,
    pub on_disconnect: D,
}

pub struct Connection {
    pub id: usize,
    pub inner: RefCell<TcpStream>,
    handler: RefCell<Box<ConnectionHandler>>,
    rbuf: RefCell<Vec<u8>>,
    wbuf: RefCell<Vec<u8>>,
}

pub fn connection_write<F>(id: usize, f: F)
    where F: FnOnce(&mut Vec<u8>)
{
    super::SLAB.with(|slab| {
        let slab = slab.borrow();
        if let Some(&super::Context::Connection(ref connection)) = slab.get(id) {
            let was_empty;
            {
                let mut wbuf = connection.wbuf.borrow_mut();
                was_empty = wbuf.is_empty();
                f(&mut wbuf);
            }
            if was_empty {
                connection.do_write(false);
            }
        }
    })
}

impl Connection {
    pub fn new_registered<H>(id: usize, stream: TcpStream, handler: H)
                             -> Result<Self, Error>
        where H: 'static + ConnectionHandler,
    {
        super::poll_register(id, &stream, Ready::readable(), PollOpt::edge());
        let conn = Connection {
            id,
            rbuf: RefCell::new(Vec::with_capacity(READ_SIZE)),
            wbuf: RefCell::new(Vec::with_capacity(0)),
            inner: RefCell::new(stream),
            handler: RefCell::new(Box::new(handler)),
        };
        Ok(conn)
    }

    fn do_write(&self, from_ready: bool) {
        let mut wbuf = self.wbuf.borrow_mut();
        let mut stream = self.inner.borrow_mut();
        while !wbuf.is_empty() {
            match stream.write(&wbuf[..]) {
                Ok(n) => {
                    wbuf.drain(..n);
                }
                Err(ref err) if err.kind() == WouldBlock => {
                    break;
                }
                Err(err) => {
                    error!("connection {}: {:?}", self.id, err);
                    super::del(self.id).unwrap();
                    let mut handler = self.handler.borrow_mut();
                    handler.on_disconnect(self.id, Some(err.into()));
                    return;
                }
            }
        }
        if from_ready && wbuf.is_empty() {
            super::poll_reregister(self.id, &*stream, Ready::readable(), PollOpt::edge());
        } else if !from_ready && !wbuf.is_empty() {
            super::poll_reregister(self.id, &*stream, Ready::readable() | Ready::writable(), PollOpt::edge());
        }
    }

    pub fn do_read(&self) -> Result<Option<usize>, io::Error> {
        let mut total_read = 0;
        {
            let mut rbuf = self.rbuf.borrow_mut();
            let mut stream = self.inner.borrow_mut();
            loop {
                rbuf.reserve(READ_SIZE);
                let orig_len = rbuf.len();
                unsafe { rbuf.set_len(orig_len + READ_SIZE) };
                match stream.read(&mut rbuf[orig_len..]) {
                    Ok(0) => {
                        error!("connection {}: remote side closed write", self.id);
                        unsafe { rbuf.set_len(orig_len) };
                        if total_read == 0 {
                            // remote side closed and we did not read anything
                            return Ok(None);
                        }
                        return Ok(Some(total_read));
                    }
                    Ok(n) => {
                        unsafe { rbuf.set_len(orig_len + n) };
                        total_read += n;
                    }
                    Err(ref err) if err.kind() == WouldBlock => {
                        unsafe { rbuf.set_len(orig_len) };
                        return Ok(Some(total_read));
                    }
                    Err(err) => {
                        error!("connection {}: {:?}", self.id, err);
                        unsafe { rbuf.set_len(orig_len) };
                        return Err(err);
                    }
                }
            }
        }
    }

    pub fn ready(&self, ready: Ready) {
        if ready.is_readable() {
            match self.do_read() {
                Ok(None) => {
                    // remote side closed write and we didn't read anything
                    super::del(self.id).unwrap();
                    let mut handler = self.handler.borrow_mut();
                    handler.on_disconnect(self.id, None);
                    return;
                }
                Ok(Some(0)) => (), // we didn't read anything this time
                Ok(Some(_n)) => {
                    // we did read something
                    let mut handler = self.handler.borrow_mut();
                    handler.on_read(self.id, &mut self.rbuf.borrow_mut());
                }
                Err(err) => {
                    super::del(self.id).unwrap();
                    let mut handler = self.handler.borrow_mut();
                    handler.on_disconnect(self.id, Some(err.into()));
                    return;
                }
            }
        }
        if ready.is_writable() {
            self.do_write(true);
        }
    }
}
