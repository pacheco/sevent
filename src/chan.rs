use std::io;
use std::cell::RefCell;
use std::sync::mpsc::TryRecvError;

use mio::Ready;
use mio::Poll;

use mio_more::channel;

// we need the trait because we want to be able to store channels of any T.
pub trait Chan {
    fn id(&self) -> usize;
    fn ready(&self, ready: Ready);
    fn deregister(&self, poll: &Poll) -> Result<(), io::Error>;
}

pub trait ChanHandler<T> {
    fn on_recv(&mut self, id: usize, msg: T);
    fn on_close(&mut self, id: usize);
}

impl<F, T> ChanHandler<T> for F
    where F: FnMut(usize, Option<T>)
{
    fn on_recv(&mut self, id: usize, msg: T) {
        self(id, Some(msg))
    }
    fn on_close(&mut self, id: usize) {
        self(id, None)
    }
}

impl<T> Chan for ChanCtx<T> {
    fn id(&self) -> usize {
        self.id
    }
    fn ready(&self, _ready: Ready) {
        loop {
            let recv = self.inner.borrow_mut().try_recv();
            match recv {
                Ok(msg) => {
                    self.handler.borrow_mut().on_recv(self.id, msg);
                }
                Err(TryRecvError::Empty) => {
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    self.handler.borrow_mut().on_close(self.id);
                    super::del(self.id).unwrap();
                    break;
                }
            }
        }
    }
    fn deregister(&self, poll: &Poll) -> Result<(), io::Error> {
        poll.deregister(&*self.inner.borrow())
    }
}

pub struct ChanCtx<T> {
    pub id: usize,
    pub inner: RefCell<channel::Receiver<T>>,
    pub handler: RefCell<Box<ChanHandler<T>>>,
}
