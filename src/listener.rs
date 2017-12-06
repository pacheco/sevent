use std::net::SocketAddr;
use std::cell::RefCell;
use std::io::ErrorKind::WouldBlock;

use mio::Ready;
use mio::net::TcpListener;
use mio::net::TcpStream;

use ::TokenKind;
use ::Error;

pub struct Listener {
    pub id: usize,
    pub inner: RefCell<TcpListener>,
    pub handler: RefCell<Box<AcceptHandler>>,
}

impl Listener {
    pub fn ready(&self, ready: Ready) {
        if ready.is_readable() {
            loop {
                let res = {
                    let listener = self.inner.borrow();
                    match listener.accept() {
                        Err(ref err) if err.kind() == WouldBlock => {
                            return;
                        }
                        res => res,
                    }
                };
                match res {
                    Ok((stream, addr)) => {
                        self.handler.borrow_mut().on_accept(Ok((stream, addr)));
                    }
                    Err(err) => {
                        debug!("listener {} accept error: {:?}", self.id, err);
                        self.handler.borrow_mut().on_accept(Err(err.into()));
                        super::del(self.id, TokenKind::Listener).unwrap();
                    }
                }
            }
        }
    }
}

pub trait AcceptHandler {
    fn on_accept(&mut self, res: Result<(TcpStream, SocketAddr), Error>);
}

impl<F> AcceptHandler for F
    where F: FnMut(Result<(TcpStream, SocketAddr), Error>)
{
    fn on_accept(&mut self, res: Result<(TcpStream, SocketAddr), Error>) {
        self(res)
    }
}
