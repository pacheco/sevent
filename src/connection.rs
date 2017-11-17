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
    /// called whenever there is new data in the connection's read buffer.
    /// The default implementation simply discards read data.
    #[allow(unused)]
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        buf.drain(..);
    }
    /// called when the connection is disconnected. There is no need
    /// to call del: the connection automatically deletes itself on
    /// disconnect.
    /// IMPORTANT: it is NOT called when the connection is explicitly deleted.
    #[allow(unused)]
    fn on_disconnect(&mut self, id: usize, error: Option<Error>) {}
    /// called whenever the connection completely empties is write buffer.
    #[allow(unused)]
    fn on_write_finished(&mut self, id: usize) {}
}

pub struct ConnectionHandlerClosures<R,D,W>
    where R: FnMut(usize, &mut Vec<u8>),
          D: FnMut(usize, Option<Error>),
          W: FnMut(usize),
{
    pub on_read: R,
    pub on_disconnect: D,
    pub on_write_finished: W,
}

impl<R, D, W> ConnectionHandler for ConnectionHandlerClosures<R,D,W>
    where R: FnMut(usize, &mut Vec<u8>),
          D: FnMut(usize, Option<Error>),
          W: FnMut(usize),
{
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        (self.on_read)(id, buf)
    }

    fn on_disconnect(&mut self, id: usize, error: Option<Error>) {
        (self.on_disconnect)(id, error)
    }

    fn on_write_finished(&mut self, id: usize) {
        (self.on_write_finished)(id)
    }
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
                connection.register_write();
            }
        } else {
            panic!("not a valid connection id!");
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

    fn register_write(&self) {
        let stream = self.inner.borrow();
        super::poll_reregister(self.id, &*stream, Ready::readable() | Ready::writable(), PollOpt::edge());
    }

    fn do_write(&self) {
        let wbuf_empty;
        let mut write_err = None;
        {
            let mut stream = self.inner.borrow_mut();
            let mut wbuf = self.wbuf.borrow_mut();
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
                        write_err = Some(err);
                        break;
                    }
                }
            }
            wbuf_empty = wbuf.is_empty();
        }

        if let Some(err) = write_err {
            super::del(self.id).unwrap();
            let mut handler = self.handler.borrow_mut();
            handler.on_disconnect(self.id, Some(err.into()));
            return;
        }

        if wbuf_empty {
            {
                let stream = self.inner.borrow_mut();
                super::poll_reregister(self.id, &*stream, Ready::readable(), PollOpt::edge());
            }
            let mut handler = self.handler.borrow_mut();
            handler.on_write_finished(self.id);
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
            self.do_write();
        }
    }
}
