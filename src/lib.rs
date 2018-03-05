#[macro_use]
extern crate log;
extern crate mio;
extern crate lazycell;
extern crate slab;
extern crate bytes;
extern crate mio_more;
extern crate bincode;
#[macro_use]
extern crate serde_derive;
extern crate serde;
extern crate rand;

extern crate iovec;
extern crate libc;

use rand::Rng;

pub mod errors;
pub use errors::Error;

pub mod iobuf;

pub mod utils;

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

pub mod ext;

use std::time::Duration;
use std::time::Instant;
use std::net::SocketAddr;
use std::cell::RefCell;
use std::rc::Rc;
use std::rc::Weak;
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

use ext::DurationExt;

const TOKEN_KIND_BITS: usize = 3;
const TOKEN_KIND_MASK: usize = 0b111;

const DEFAULT_POLL_TIMEOUT_NS: u32 = 10_000;
const DEFAULT_MAX_READ_SIZE: usize = 16*1024;
const DEFAULT_MAX_WRITE_SIZE: usize = 16*1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub randomize_work: bool,
    pub max_read_size: usize,
    pub max_write_size: usize,
    pub work_until_would_block: bool,
    pub max_loop_exec_time_us: Option<u64>,
    pub poll_timeout_ns: u32,
}

impl std::default::Default for Config {
    fn default() -> Self {
        Config {
            /// Time to block waiting for new events on poll (for when there are still pending events)
            poll_timeout_ns: DEFAULT_POLL_TIMEOUT_NS,
            /// Randomize order in which connections are handled at each loop tick
            randomize_work: true,
            /// Whether all events should be handled before polling for more
            work_until_would_block: false,
            /// Set a time limit for loop execution before checking for more events
            max_loop_exec_time_us: None,
            /// Maximum amount of bytes to read at each "read" syscall
            max_read_size: DEFAULT_MAX_READ_SIZE,
            /// Maximum amount of bytes to write at each "write" syscall
            max_write_size: DEFAULT_MAX_WRITE_SIZE,
        }
    }
}

#[derive(Clone)]
enum ReadyCtx {
    Read(Weak<Connection>),
    Write(Weak<Connection>),
    Done,
}

struct LoopCtx {
    poll: Poll,
    shutdown: RefCell<bool>,
    max_pending_write: RefCell<usize>,
    max_loop_exec_time: RefCell<Duration>,
    listeners: RefCell<Slab<Rc<Listener>>>,
    connects: RefCell<Slab<Rc<Connect>>>,
    timer: RefCell<mio_timer::Timer<Box<TimeoutHandler>>>,
    chans: RefCell<Slab<Rc<Chan>>>,
    conns: RefCell<Slab<Rc<Connection>>>,
    conns_ready: RefCell<Vec<ReadyCtx>>,
}

thread_local! {
    static CTX: LazyCell<LoopCtx> = LazyCell::new();
    static CFG: RefCell<Config> = RefCell::default();
}

pub fn run_evloop_with_config<F, E>(config: Config, init: F) -> Result<(), E>
    where E: std::fmt::Debug + From<std::io::Error>,
          F: FnOnce() -> Result<(), E>
{
    CFG.with(|cfg| *cfg.borrow_mut() = config);
    run_evloop(init)
}

pub fn run_evloop<F, E: std::fmt::Debug>(init: F) -> Result<(), E>
    where E: std::fmt::Debug + From<std::io::Error>,
          F: FnOnce() -> Result<(), E>
{
    CTX.with(|ctx| -> Result<(), E> {
        // create the LoopCtx
        let ctx = ctx.borrow_with(|| {
            LoopCtx {
                poll: Poll::new().expect("could not create event poll"),
                shutdown: RefCell::new(false),
                max_pending_write: RefCell::new(0),
                max_loop_exec_time: RefCell::new(Duration::from_secs(0)),
                listeners: RefCell::new(Slab::new()),
                connects: RefCell::new(Slab::new()),
                timer: RefCell::new(mio_timer::Timer::default()),
                chans: RefCell::new(Slab::new()),
                conns: RefCell::new(Slab::new()),
                conns_ready: RefCell::new(Vec::new()),
            }
        });

        // register timer
        poll_register(0, TokenKind::Timer, &*ctx.timer.borrow(), Ready::readable(), PollOpt::edge());

        // user initialization
        init()?;

        // at every loop tick, we don't write/read until
        // wouldblock. Instead, whenever there are connections already
        // writeable/readable, we don't do a blocking poll.
        let mut pending_writes_or_reads = false;

        let mut events = Events::with_capacity(1024);
        let cfg = CFG.with(|cfg| cfg.borrow().clone());

        while !*ctx.shutdown.borrow() {
            let poll_timeout = if pending_writes_or_reads {
                Some(Duration::new(0, cfg.poll_timeout_ns))
            } else {
                None
            };

            match ctx.poll.poll(&mut events, poll_timeout) {
                Ok(_) => (),
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(E::from(err)),
            }
            let loop_start = Instant::now();

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
                        let was_readable = conn.is_readable();
                        let was_writable = conn.is_writable();
                        conn.ready(event.readiness());
                        if !was_readable && conn.is_readable() {
                            ctx.conns_ready.borrow_mut().push(ReadyCtx::Read(Rc::downgrade(&conn)));
                        }
                        if !was_writable && conn.is_writable() {
                            ctx.conns_ready.borrow_mut().push(ReadyCtx::Write(Rc::downgrade(&conn)));
                        }
                    }
                }
            }

            // handle ready connections
            loop {
                let rcnt = ctx.conns_ready.borrow().len();
                if cfg.randomize_work {
                    let mut rconn = ctx.conns_ready.borrow_mut();
                    rand::thread_rng().shuffle(&mut *rconn);
                }
                let mut exec_time_limit = false;
                for i in 0 .. rcnt {
                    // TODO: work on one connection at least?
                    if let Some(limit) = cfg.max_loop_exec_time_us {
                        if Instant::now().duration_since(loop_start).as_usecs() > limit {
                            exec_time_limit = true;
                            break;
                        }
                    }

                    let ready = { ctx.conns_ready.borrow_mut()[i].clone() };
                    match ready {
                        ReadyCtx::Read(wconn) => {
                            if let Some(conn) = wconn.upgrade() {
                                if conn.do_read() {
                                    pending_writes_or_reads = true;
                                    continue;
                                }
                            }
                            *ctx.conns_ready.borrow_mut().get_mut(i).unwrap() = ReadyCtx::Done;
                        }
                        ReadyCtx::Write(wconn) => {
                            if let Some(conn) = wconn.upgrade() {
                                if conn.do_write() {
                                    if *ctx.max_pending_write.borrow() < conn.wbuf.borrow().len() {
                                        *ctx.max_pending_write.borrow_mut() = conn.wbuf.borrow().len();
                                        debug!("MAX PENDING WRITE: {:?}", ctx.max_pending_write.borrow());
                                    }
                                    pending_writes_or_reads = true;
                                    continue;
                                }
                            }
                            *ctx.conns_ready.borrow_mut().get_mut(i).unwrap() = ReadyCtx::Done;
                        }
                        ReadyCtx::Done => continue,
                    }
                }

                let mut conns_ready = ctx.conns_ready.borrow_mut();
                conns_ready.retain(|it| {
                    match *it {
                        ReadyCtx::Done => false,
                        _ => true,
                    }
                });

                if exec_time_limit {
                    break;
                }

                if !cfg.work_until_would_block {
                    break;
                }

                if conns_ready.len() == 0 {
                    break;
                }
            }

            let exec_time = Instant::now().duration_since(loop_start);
            {
                let mut max_exec_time = ctx.max_loop_exec_time.borrow_mut();
                if exec_time > *max_exec_time {
                    debug!("MAX LOOP EXEC TIME: {}", exec_time.as_nanosecs());
                    *max_exec_time = exec_time;
                }
            }
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

pub fn set_timeout<H: 'static + TimeoutHandler>(after: Duration, handler: H) -> Result<mio_timer::Timeout, Error> {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        ctx.timer.borrow_mut().set_timeout(after, Box::new(handler)).map_err(|e| e.into())
    })
}

pub fn cancel_timeout(timeout: &mio_timer::Timeout) -> Option<Box<TimeoutHandler>> {
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        ctx.timer.borrow_mut().cancel_timeout(timeout)
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

pub fn add_connection<H>(stream: TcpStream, handler: H) -> Result<usize, Error>
    where H: 'static + ConnectionHandler,
{
    CTX.with(|ctx| {
        let ctx = ctx.borrow().expect("not inside evloop");
        let id;
        let conn = {
            let mut slab = ctx.conns.borrow_mut();
            let e = slab.vacant_entry();
            id = e.key();
            poll_register(id, TokenKind::Connection, &stream,
                          Ready::readable() | Ready::writable(), PollOpt::edge());
            let conn = Connection::new(id, stream, handler)?;
            e.insert(Rc::new(conn)).clone()
        };
        conn.handler_on_add();
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
            addr,
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
