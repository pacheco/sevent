use std::cell::RefCell;
use std::rc::Rc;

use mio::Ready;
use mio::net::TcpStream;

use ::Error;

pub trait ConnectHandler {
    fn on_result(&mut self, result: Result<TcpStream, Error>);
}



impl<F> ConnectHandler for F
    where F: FnMut(Result<TcpStream, Error>)
{
    fn on_result(&mut self, result: Result<TcpStream, Error>) {
        self(result)
    }
}

pub struct Connect {
    pub id: usize,
    pub inner: RefCell<TcpStream>,
    pub handler: RefCell<Box<ConnectHandler>>,
}

impl Connect {
    pub fn ready(self_rc: Rc<Self>, ready: Ready) {
        if ready.is_writable() {
            let res_err = self_rc.inner.borrow().take_error();
            match res_err {
                Ok(Some(err)) => {
                    super::del(self_rc.id).unwrap();
                    self_rc.handler.borrow_mut().on_result(Err(err.into()));
                }
                Ok(None) => {
                    super::del(self_rc.id).unwrap();
                    match Rc::try_unwrap(self_rc) {
                        Ok(connect) => {
                            let stream = connect.inner.into_inner();
                            let mut handler = connect.handler.into_inner();
                            handler.on_result(Ok(stream));
                        }
                        Err(_) => panic!("there should be no other rcs!"),
                    }
                }
                Err(err) => panic!(err),
            }
        }
    }
}
