#[macro_use]
extern crate log;
extern crate mio;
extern crate lazycell;
extern crate slab;
extern crate bytes;
extern crate mio_more;
extern crate bincode;
extern crate serde;

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

mod timer;
pub use self::timer::TimeoutHandler;

mod tick;
pub use self::tick::TickHandler;

pub mod ext;

use std::time::Duration;
use std::net::SocketAddr;
use std::cell::RefCell;
use std::rc::Rc;
use std::rc::Weak;
use std::collections::VecDeque;
use std::mem;
use std::io;

use mio::Events;
use mio::Poll;
use mio::Ready;
use mio::PollOpt;
use mio::Token;
use mio::net::TcpListener;
use mio::net::TcpStream;
use mio::event::Evented;

use mio_more::channel;
use mio_more::timer as mio_timer;

use slab::Slab;

use lazycell::LazyCell;

const TOKEN_KIND_BITS: usize = 3;
const TOKEN_KIND_MASK: usize = 0b111;

struct LoopCtx {
    poll: Poll,
    shutdown: RefCell<bool>,
    listeners: RefCell<Slab<Rc<Listener>>>,
    connects: RefCell<Slab<Rc<Connect>>>,
    timer: RefCell<mio_timer::Timer<Box<TimeoutHandler>>>,
    chans: RefCell<Slab<Rc<Chan>>>,
    conns: RefCell<Slab<Rc<Connection>>>,
    conns_readable: RefCell<VecDeque<Weak<Connection>>>,
    conns_writable: RefCell<VecDeque<Weak<Connection>>>,
    on_current_tick: RefCell<VecDeque<Box<TickHandler>>>,
    on_next_tick: RefCell<VecDeque<Box<TickHandler>>>,
}

thread_local! {
    static CTX: LazyCell<LoopCtx> = LazyCell::new();
}

#[derive(Clone)]
pub enum Context {
    // Connection(Rc<Connection>),
    // Connect(Rc<Connect>),
    // Listener(Rc<Listener>),
    // Chan(Rc<Chan>),
    // Timer(Rc<RefCell<mio_timer::Timer<Box<TimeoutHandler>>>>),
}

pub fn run_evloop<F>(init: F) -> Result<(), Error>
    where F: FnOnce() -> Result<(), Error>
{
    CTX.with(|ctx| -> Result<(), Error> {
        // create the LoopCtx
        let ctx = ctx.borrow_with(|| {
            LoopCtx {
                poll: Poll::new().expect("could not create event poll"),
                shutdown: RefCell::new(false),
                listeners: RefCell::new(Slab::new()),
                connects: RefCell::new(Slab::new()),
                timer: RefCell::new(mio_timer::Timer::default()),
                chans: RefCell::new(Slab::new()),
                conns: RefCell::new(Slab::new()),
                conns_readable: RefCell::new(VecDeque::new()),
                conns_writable: RefCell::new(VecDeque::new()),
                on_current_tick: RefCell::new(VecDeque::new()),
                on_next_tick: RefCell::new(VecDeque::new()),
            }
        });

        // register timer
        poll_register(0, TokenKind::Timer, &*ctx.timer.borrow(), Ready::readable(), PollOpt::edge());

        // user initialization
        init()?;

        let mut events = Events::with_capacity(1024);
        while !*ctx.shutdown.borrow() {
            match ctx.poll.poll(&mut events, None) {
                Ok(_) => (),
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(Error::from(err)),
            }
            for event in &events {
                match event.token().to_id_kind() {
                    (id, TokenKind::Listener) => {
                        let listener = {
                            ctx.listeners.borrow().get(id).cloned().expect("invalid token")
                        };
                        listener.ready(event.readiness());
                    }
                    (id, TokenKind::Connect) => {
                        let connect = {
                            ctx.connects.borrow().get(id).cloned().expect("invalid token")
                        };
                        Connect::ready(connect, event.readiness());
                    }
                    (id, TokenKind::Chan) => {
                        let chan = {
                            ctx.chans.borrow().get(id).cloned().expect("invalid token")
                        };
                        chan.ready(event.readiness());
                    }
                    (_id, TokenKind::Timer) => {
                        loop {
                            let tev = { ctx.timer.borrow_mut().poll() };
                            if let Some(timeout) = tev {
                                timeout.on_timeout();
                            } else {
                                break;
                            }
                        }
                    }
                    (id, TokenKind::Connection) => {
                        let conn = {
                            ctx.conns.borrow().get(id).cloned().expect("invalid token")
                        };
                        // place connections with work to be done in the read/write queues
                        conn.ready(event.readiness());
                        if conn.is_readable() {
                            ctx.conns_readable.borrow_mut().push_back(Rc::downgrade(&conn));
                        }
                        if conn.is_writable() {
                            ctx.conns_writable.borrow_mut().push_back(Rc::downgrade(&conn));
                        }
                    }
                }
            }
            // reading, writing and ticks may trigger a handler, which
            // may add another write/tick, so we loop here until
            // there's nothing to do
            while {
                !(ctx.conns_readable.borrow().is_empty()
                  && ctx.on_current_tick.borrow().is_empty()
                  && ctx.conns_writable.borrow().is_empty())
            } {
                // The weird structure of the loops here is because
                // the borrow checker is too conservative

                // handle reads
                loop {
                    let wconn = { ctx.conns_readable.borrow_mut().pop_front() };
                    if let Some(conn) = wconn.and_then(|wconn| wconn.upgrade()) {
                        if conn.do_read() {
                            ctx.conns_readable.borrow_mut().push_back(Rc::downgrade(&conn));
                        }
                    } else {
                        break;
                    }
                }
                // handle writes
                loop {
                    let wconn = { ctx.conns_writable.borrow_mut().pop_front() };
                    if let Some(conn) = wconn.and_then(|wconn| wconn.upgrade()) {
                        if conn.do_write() {
                            ctx.conns_writable.borrow_mut().push_back(Rc::downgrade(&conn));
                        }
                    } else {
                        break;
                    }
                }
                // handle current tick events
                loop {
                    let ev = { ctx.on_current_tick.borrow_mut().pop_front() };
                    if let Some(ev) = ev {
                        ev.on_tick();
                    } else {
                        break;
                    }
                }
            }
            // swap current/next tick events for the next loop
            mem::swap(&mut *ctx.on_current_tick.borrow_mut(), &mut *ctx.on_next_tick.borrow_mut());
        }
        Ok(())
    })
}

pub fn shutdown() {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        *ctx.shutdown.borrow_mut() = true;
    })
}

pub fn on_current_tick<H: 'static + TickHandler>(handler: H) {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        ctx.on_current_tick.borrow_mut().push_back(Box::new(handler));
    })
}

pub fn on_next_tick<H: 'static + TickHandler>(handler: H) {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        ctx.on_next_tick.borrow_mut().push_back(Box::new(handler));
    })
}

pub fn set_timeout<H: 'static + TimeoutHandler>(after: Duration, handler: H) -> Result<mio_timer::Timeout, Error> {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        ctx.timer.borrow_mut().set_timeout(after, Box::new(handler)).map_err(|e| e.into())
    })
}

pub fn cancel_timeout(timeout: &mio_timer::Timeout) -> Option<Box<TimeoutHandler>> {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        ctx.timer.borrow_mut().cancel_timeout(&timeout)
    })
}

pub fn add_listener<H: 'static + AcceptHandler>(listener: TcpListener, handler: H) -> Result<usize, Error> {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        let mut slab = ctx.listeners.borrow_mut();
        let e = slab.vacant_entry();
        let id = e.key();
        poll_register(id, TokenKind::Listener, &listener, Ready::readable(), PollOpt::edge());
        let listener = Listener {
            id,
            inner: RefCell::new(listener),
            handler: RefCell::new(Box::new(handler)),
        };
        e.insert(Rc::new(listener));
        Ok(id)
    })
}

pub fn add_connection<H: 'static + ConnectionHandler>(stream: TcpStream, handler: H) -> Result<usize, Error> {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        let mut slab = ctx.conns.borrow_mut();
        let e = slab.vacant_entry();
        let id = e.key();
        poll_register(id, TokenKind::Connection, &stream,
                      Ready::readable() | Ready::writable(), PollOpt::edge());
        let conn = Connection::new(id, stream, handler)?;
        e.insert(Rc::new(conn));
        Ok(id)
    })
}

pub fn add_connect<H: 'static + ConnectHandler>(addr: SocketAddr, handler: H) -> Result<usize, Error> {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        let mut slab = ctx.connects.borrow_mut();
        let e = slab.vacant_entry();
        let id = e.key();
        let stream = TcpStream::connect(&addr)?;
        poll_register(id, TokenKind::Connect, &stream, Ready::writable(), PollOpt::edge());
        let connect = Connect {
            id,
            addr: addr,
            inner: RefCell::new(stream),
            handler: RefCell::new(Box::new(handler)),
        };
        e.insert(Rc::new(connect));
        Ok(id)
    })
}

pub fn add_chan<H, T>(chan: channel::Receiver<T>, handler: H) -> Result<usize, Error>
    where T: 'static,
          H: 'static + ChanHandler<T>
{
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        let mut slab = ctx.chans.borrow_mut();
        let e = slab.vacant_entry();
        let id = e.key();
        poll_register(id, TokenKind::Chan, &chan, Ready::readable(), PollOpt::edge());
        let chan = ChanCtx {
            id,
            inner: RefCell::new(chan),
            handler: RefCell::new(Box::new(handler)),
        };
        e.insert(Rc::new(chan));
        Ok(id)
    })
}

pub fn del(id: usize, kind: TokenKind) -> Result<(), Error> {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        match kind {
            TokenKind::Chan => {
                let mut slab = ctx.chans.borrow_mut();
                if slab.contains(id) {
                    slab.remove(id).deregister(&ctx.poll).expect("error deregistering");
                    Ok(())
                } else {
                    Err(Error::InvalidId)
                }
            }
            TokenKind::Connect => {
                let mut slab = ctx.connects.borrow_mut();
                if slab.contains(id) {
                    let connect = slab.remove(id);
                    poll_deregister(&*connect.inner.borrow());
                    Ok(())
                } else {
                    Err(Error::InvalidId)
                }
            }
            TokenKind::Connection => {
                let mut slab = ctx.conns.borrow_mut();
                if slab.contains(id) {
                    let connection = slab.remove(id);
                    poll_deregister(&*connection.inner.borrow());
                    Ok(())
                } else {
                    Err(Error::InvalidId)
                }
            }
            TokenKind::Listener => {
                let mut slab = ctx.listeners.borrow_mut();
                if slab.contains(id) {
                    let listener = slab.remove(id);
                    poll_deregister(&*listener.inner.borrow());
                    Ok(())
                } else {
                    Err(Error::InvalidId)
                }
            }
            TokenKind::Timer => {
                panic!("cannot delete global timer");
            }
        }
    })
}

fn poll_deregister<E: Evented>(evented: &E) {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        ctx.poll.deregister(evented).expect("error deregistering from evloop");
    })
}

fn poll_register<E: Evented>(id: usize, kind: TokenKind, evented: &E, ready: Ready, opt: PollOpt) {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        ctx.poll.register(evented, Token::from_id_kind(id, kind), ready, opt).expect("error registering to evloop");
    })
}

#[repr(u8)]
#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum TokenKind {
    Listener,
    Connect,
    Connection,
    Chan,
    Timer,
}

trait TokenExt {
    fn from_id_kind(id: usize, kind: TokenKind) -> Self;
    fn to_id_kind(&self) -> (usize, TokenKind);
}

impl TokenExt for Token {
    fn to_id_kind(&self) -> (usize, TokenKind) {
        let kind_id = (self.0 & TOKEN_KIND_MASK) as u8;
        let kind = unsafe { mem::transmute(kind_id) };
        (self.0 >> TOKEN_KIND_BITS, kind)
    }

    fn from_id_kind(id: usize, kind: TokenKind) -> Self {
        Token((id << TOKEN_KIND_BITS) | (kind as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enum_to_int() {
        for kind in vec![TokenKind::Listener,
                         TokenKind::Connection,
                         TokenKind::Connect,
                         TokenKind::Chan,
                         TokenKind::Timer] {
            assert_eq!((0, kind), Token::from_id_kind(0, kind).to_id_kind());
            assert_eq!((123, kind), Token::from_id_kind(123, kind).to_id_kind());
        }
    }
}

