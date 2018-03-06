use std::cmp::min;
use std::cell::RefCell;
use std::io::ErrorKind::WouldBlock;
use std::io::Read;
use std::rc::Rc;

use bytes::Buf;
use bytes::BufMut;

use mio::Ready;
use mio::net::TcpStream;

use ::iobuf::IoBuffer;

use ::ReadyCtx;
use ::TokenKind;
use ::Error;

pub trait ConnectionHandler {
    /// called right after the connection is added to the event loop with `add_connection`.
    #[allow(unused)]
    fn on_add(&mut self, id: usize) {}
    /// called whenever there is new data in the connection's read buffer.
    /// The default implementation simply discards read data.
    #[allow(unused)]
    fn on_read(&mut self, id: usize, buf: &mut IoBuffer) {
        let remaining = buf.remaining();
        buf.advance(remaining);
    }
    /// called when the connection is disconnected. There is no need
    /// to call del: the connection automatically deletes itself on
    /// disconnect.
    /// IMPORTANT: it is NOT called after a `sevent::del` for the connection
    #[allow(unused)]
    fn on_disconnect(&mut self, id: usize, error: Option<Error>) {}
    /// called whenever the connection completely empties is write buffer.
    #[allow(unused)]
    fn on_write_finished(&mut self, id: usize) {}
}

pub struct ConnectionHandlerClosures<R,D,W>
    where R: FnMut(usize, &mut IoBuffer),
          D: FnMut(usize, Option<Error>),
          W: FnMut(usize),
{
    // TODO: on_add
    pub on_read: R,
    pub on_disconnect: D,
    pub on_write_finished: W,
}

impl<R, D, W> ConnectionHandler for ConnectionHandlerClosures<R,D,W>
    where R: FnMut(usize, &mut IoBuffer),
          D: FnMut(usize, Option<Error>),
          W: FnMut(usize),
{
    fn on_read(&mut self, id: usize, buf: &mut IoBuffer) {
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
    pub ready: RefCell<Ready>,
    pub rbuf: RefCell<IoBuffer>,
    pub wbuf: RefCell<IoBuffer>,
    handler: RefCell<Box<ConnectionHandler>>,
}

pub fn connection_write<F>(id: usize, f: F) -> Result<(), Error>
    where F: FnOnce(&mut IoBuffer),
{
    super::CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        match ctx.conns.borrow().get(id) {
            Some(conn) => {
                let was_writable = conn.is_writable();
                f(&mut conn.wbuf.borrow_mut());
                if conn.is_writable() && !was_writable {
                    ctx.conns_ready.borrow_mut().push(ReadyCtx::Write(Rc::downgrade(conn)));
                }
                Ok(())
            }
            None => Err(Error::InvalidId),
        }
    })
}

impl Connection {
    pub fn new<H>(id: usize,
                  stream: TcpStream,
                  handler: H)
                  -> Result<Self, Error>
        where H: 'static + ConnectionHandler,
    {
        super::CFG.with(|cfg| {
            let conn = Connection {
                id,
                ready: RefCell::new(Ready::writable()),
                rbuf: RefCell::new(IoBuffer::with_capacity(cfg.borrow().max_read_size)),
                wbuf: RefCell::new(IoBuffer::with_capacity(cfg.borrow().max_write_size)),
                inner: RefCell::new(stream),
                handler: RefCell::new(Box::new(handler)),
            };
            Ok(conn)
        })
    }

    pub fn handler_on_add(&self) {
        self.handler.borrow_mut().on_add(self.id);
    }

    pub fn is_writable(&self) -> bool {
        self.ready.borrow().is_writable() && self.to_write() > 0
    }

    fn to_write(&self) -> usize {
        self.wbuf.borrow().remaining()
    }

    pub fn is_readable(&self) -> bool {
        self.ready.borrow().is_readable()
    }

    // Tries to write some data.
    // Returns true if the connection is still ready for another
    // write, i.e.,: no errors, has data to be written and would not block
    pub fn do_write(&self) -> bool {
        assert!(!self.to_write() > 0);
        let mut write_err = None;
        {
            let stream = self.inner.borrow_mut();
            let mut wbuf = self.wbuf.borrow_mut();
            let to_advance = {
                // let bytes = wbuf.bytes();
                // let max_write_size = super::CFG.with(|cfg| cfg.borrow().max_write_size);
                // let up_to = min(max_write_size, bytes.len());
                match wbuf.writev_to(&*stream) {
                    Ok(n) => {
                        n
                    }
                    Err(err) => {
                        write_err = Some(err);
                        0
                    }
                }
            };
            wbuf.advance(to_advance);
        }

        if let Some(err) = write_err {
            if err.kind() == WouldBlock {
                self.ready.borrow_mut().remove(Ready::writable());
                return false;
            } else {
                debug!("connection {}: {:?}", self.id, err);
                super::del(self.id, TokenKind::Connection).unwrap();
                let mut handler = self.handler.borrow_mut();
                handler.on_disconnect(self.id, Some(err.into()));
                return false;
            }
        }

        if self.to_write() == 0 {
            let mut handler = self.handler.borrow_mut();
            handler.on_write_finished(self.id);
            false
        } else {
            true
        }
    }

    // Tries to read some data.  Returns true if the connection is
    // still ready for another read, i.e.,: no errors and would not
    // block.
    pub fn do_read(&self) -> bool {
        let mut rbuf = self.rbuf.borrow_mut();
        match {
            let mut stream = self.inner.borrow_mut();
            let max_read_size = super::CFG.with(|cfg| cfg.borrow().max_write_size);
            rbuf.reserve(max_read_size);
            let bytes = unsafe { rbuf.bytes_mut() };
            let up_to = min(max_read_size, bytes.len());
            stream.read(&mut bytes[..up_to])
        } {
            Ok(0) => {
                // remote side closed
                debug!("connection {}: remote closed for writing", self.id);
                // TODO: should we handle partial close?
                super::del(self.id, TokenKind::Connection).unwrap();
                let mut handler = self.handler.borrow_mut();
                handler.on_disconnect(self.id, None);
                false
            }
            Ok(n) => {
                unsafe { rbuf.advance_mut(n) };
                // we read something
                let mut handler = self.handler.borrow_mut();
                handler.on_read(self.id, &mut rbuf);
                true
            }
            Err(ref err) if err.kind() == WouldBlock => {
                self.ready.borrow_mut().remove(Ready::readable());
                false
            }
            Err(err) => {
                debug!("connection {}: {:?}", self.id, err);
                super::del(self.id, TokenKind::Connection).unwrap();
                let mut handler = self.handler.borrow_mut();
                handler.on_disconnect(self.id, Some(err.into()));
                false
            }
        }
    }

    pub fn ready(&self, ready: Ready) {
        self.ready.borrow_mut().insert(ready);
    }
}
