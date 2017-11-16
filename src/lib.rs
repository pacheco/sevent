#[macro_use]
extern crate log;
extern crate mio;
extern crate lazycell;
extern crate slab;
extern crate bytes;
extern crate mio_more;

pub mod errors;
pub use errors::Error;

mod connection;
use self::connection::Connection;
pub use self::connection::ConnectionHandler;
pub use self::connection::ConnectionHandlerClosures;
pub use self::connection::connection_write;
mod connect;
use self::connect::Connect;
pub use self::connect::ConnectHandler;
mod listener;
use self::listener::Listener;
pub use self::listener::AcceptHandler;
mod chan;
use self::chan::Chan;
use self::chan::ChanCtx;
pub use self::chan::ChanHandler;

use std::net::SocketAddr;
use std::cell::RefCell;
use std::rc::Rc;

use mio::Events;
use mio::Poll;
use mio::Ready;
use mio::PollOpt;
use mio::Token;
use mio::net::TcpListener;
use mio::net::TcpStream;
use mio::event::Evented;

use mio_more::channel;

use slab::Slab;

use lazycell::LazyCell;

thread_local! {
    static POLL: LazyCell<Poll> = LazyCell::new();
    static SHUTDOWN: RefCell<bool> = RefCell::new(false);
    static SLAB: RefCell<Slab<Context>> = RefCell::new(Slab::new());
}

#[derive(Clone)]
pub enum Context {
    Connection(Rc<Connection>),
    Connect(Rc<Connect>),
    Listener(Rc<Listener>),
    Chan(Rc<Chan>),
}

pub fn run_evloop<F>(init: F) -> Result<(), Error>
    where F: FnOnce() -> Result<(), Error>
{
    POLL.with(|p| -> Result<(), Error> {
        let poll = Poll::new()?;
        let poll = p.borrow_with(|| poll);
        init()?;
        let mut events = Events::with_capacity(1024);
        loop {
            let shutdown = SHUTDOWN.with(|s| *s.borrow());
            if shutdown { break; }
            poll.poll(&mut events, None)?;

            for event in events.iter() {
                let id: usize = event.token().into();
                debug!("events for {}: {:?}", id, event.readiness());
                SLAB.with(|slab| {
                    let ctx = {
                        let slab = slab.borrow();
                        slab.get(id).expect("invalid event id").clone()
                    };
                    match ctx {
                        Context::Connect(connect) => {
                            Connect::ready(connect, event.readiness());
                        }
                        Context::Connection(connection) => {
                            connection.ready(event.readiness());
                        }
                        Context::Listener(listener) => {
                            listener.ready(event.readiness());
                        }
                        Context::Chan(chan) => {
                            chan.ready(event.readiness());
                        }
                    }
                });
            }
        }
        Ok(())
    })?;
    Ok(())
}

pub fn shutdown() {
    SHUTDOWN.with(|s| {
        *s.borrow_mut() = true;
    })
}

pub fn add_listener<H: 'static + AcceptHandler>(listener: TcpListener, handler: H) -> Result<usize, Error> {
    SLAB.with(|slab| {
        let mut slab = slab.borrow_mut();
        let e = slab.vacant_entry();
        let id = e.key();
        poll_register(id, &listener, Ready::readable(), PollOpt::edge());
        let listener = Listener {
            id,
            inner: RefCell::new(listener),
            handler: RefCell::new(Box::new(handler)),
        };
        let ctx = Context::Listener(Rc::new(listener));
        e.insert(ctx);
        Ok(id)
    })
}

pub fn add_connection<H: 'static + ConnectionHandler>(stream: TcpStream, handler: H) -> Result<usize, Error> {
    SLAB.with(|slab| {
        let mut slab = slab.borrow_mut();
        let e = slab.vacant_entry();
        let id = e.key();
        let conn = Connection::new_registered(id, stream, handler)?;
        let ctx = Context::Connection(Rc::new(conn));
        e.insert(ctx);
        Ok(id)
    })
}

pub fn add_connect<H: 'static + ConnectHandler>(addr: SocketAddr, handler: H) -> Result<usize, Error> {
    SLAB.with(|slab| {
        let mut slab = slab.borrow_mut();
        let e = slab.vacant_entry();
        let id = e.key();
        let stream = TcpStream::connect(&addr)?;
        poll_register(id, &stream, Ready::writable(), PollOpt::edge());
        let connect = Connect {
            id,
            inner: RefCell::new(stream),
            handler: RefCell::new(Box::new(handler)),
        };
        let ctx = Context::Connect(Rc::new(connect));
        e.insert(ctx);
        Ok(id)
    })
}

pub fn add_chan<H, T>(chan: channel::Receiver<T>, handler: H) -> Result<usize, Error>
    where T: 'static,
          H: 'static + ChanHandler<T>
{
    SLAB.with(|slab| {
        let mut slab = slab.borrow_mut();
        let e = slab.vacant_entry();
        let id = e.key();
        poll_register(id, &chan, Ready::readable(), PollOpt::edge());
        let chan = ChanCtx {
            id,
            inner: RefCell::new(chan),
            handler: RefCell::new(Box::new(handler)),
        };
        let ctx = Context::Chan(Rc::new(chan));
        e.insert(ctx);
        Ok(id)
    })
}

pub fn del(id: usize) -> Result<(), Error> {
    SLAB.with(|slab| {
        let mut slab = slab.borrow_mut();
        if slab.contains(id) {
            POLL.with(|p| {
                let poll = p.borrow().expect("not inside evloop");
                match slab.remove(id) {
                    Context::Connect(connect) => {
                        poll.deregister(&*connect.inner.borrow()).expect("error deregistering");
                    }
                    Context::Connection(connection) => {
                        poll.deregister(&*connection.inner.borrow()).expect("error deregistering");
                    }
                    Context::Listener(listener) => {
                        poll.deregister(&*listener.inner.borrow()).expect("error deregistering");
                    }
                    Context::Chan(chan) => {
                        chan.deregister(&poll).expect("error deregistering");
                    }
                }
            });
            Ok(())
        } else {
            Err(Error::InvalidId)
        }
    })
}

fn poll_register<E: Evented>(id: usize, evented: &E, ready: Ready, opt: PollOpt) {
    println!("registering {}", id);
    POLL.with(|p| {
        let poll = p.borrow().expect("not inside evloop");
        poll.register(evented, Token(id), ready, opt).expect("error registering to evloop");
    })
}

fn poll_reregister<E: Evented>(id: usize, evented: &E, ready: Ready, opt: PollOpt) {
    println!("re-registering {}", id);
    POLL.with(|p| {
        let poll = p.borrow().expect("not inside evloop");
        poll.reregister(evented, Token(id), ready, opt).expect("error registering to evloop");
    })
}
