use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use mio::Ready;
use mio::net::TcpStream;

use ::TokenKind;
use ::Error;

pub trait ConnectHandler {
    fn on_connect_result(self: Box<Self>, addr: SocketAddr, result: Result<TcpStream, Error>);
}

impl<F> ConnectHandler for F
    where F: FnOnce(SocketAddr, Result<TcpStream, Error>)
{
    fn on_connect_result(self: Box<Self>, addr: SocketAddr, result: Result<TcpStream, Error>) {
        self(addr, result)
    }
}

pub struct Connect {
    pub id: usize,
    pub addr: SocketAddr,
    pub inner: RefCell<TcpStream>,
    pub handler: RefCell<Box<ConnectHandler>>,
}

impl Connect {
    pub fn ready(self_rc: Rc<Self>, ready: Ready) {
        if ready.is_writable() {
            let res_err = self_rc.inner.borrow().take_error();
            match res_err {
                Ok(Some(err)) => {
                    // connect failure
                    super::del(self_rc.id, TokenKind::Connect).unwrap();
                    match Rc::try_unwrap(self_rc) {
                        Ok(connect) => {
                            let handler = connect.handler.into_inner();
                            handler.on_connect_result(connect.addr, Err(err.into()));
                        }
                        Err(_) => panic!("this should be the only RC!"),
                    }
                }
                Ok(None) => {
                    // connected!
                    super::del(self_rc.id, TokenKind::Connect).unwrap();
                    match Rc::try_unwrap(self_rc) {
                        Ok(connect) => {
                            let stream = connect.inner.into_inner();
                            let handler = connect.handler.into_inner();
                            handler.on_connect_result(connect.addr, Ok(stream));
                        }
                        Err(_) => panic!("there should be no other rcs!"),
                    }
                }
                Err(err) => panic!(err),
            }
        }
    }
}
