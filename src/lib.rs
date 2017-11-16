#[macro_use]
extern crate log;
extern crate mio;
extern crate lazycell;
extern crate slab;
extern crate bytes;

pub mod errors;
pub use errors::Error;
pub mod handler;
pub mod v2;

use std::cell::RefCell;
use std::rc::Rc;

use mio::Poll;
use mio::PollOpt;
use mio::Events;
use mio::Ready;
use mio::Token;
use mio::event::Evented;

use slab::Slab;

use lazycell::LazyCell;

thread_local! {
    static POLL: LazyCell<Poll> = LazyCell::new();
    static SHUTDOWN: RefCell<bool> = RefCell::new(false);
    static HANDLERS: RefCell<Slab<Rc<RefCell<EventHandler>>>> = RefCell::new(Slab::new());
}

pub trait EventHandler: Evented {
    fn ready(&self, self_id: usize, ready: Ready);
}

pub fn register(handler: Rc<RefCell<EventHandler>>, ready: Ready, opt: PollOpt) -> Result<usize, Error> {
    HANDLERS.with(|h| {
        let id = h.borrow_mut().insert(handler);
        POLL.with(|p| {
            debug!("registering new handler {}", id);
            let poll = p.borrow().expect("outside event loop!");
            let h = h.borrow();
            let evented = h.get(id).unwrap().borrow();
            poll.register(&*evented, Token(id), ready, opt)?;
            Ok(id)
        })
    })
}

pub fn reregister(id: usize, ready: Ready, opt: PollOpt) -> Result<(), Error> {
    HANDLERS.with(|h| {
        let h = h.borrow();
        let handler = h.get(id).ok_or(Error::InvalidId)?;
        POLL.with(|p| {
            debug!("reregistering handler {}", id);
            let poll = p.borrow().expect("outside event loop!");
            poll.reregister(&*handler.borrow(), Token(id), ready, opt)?;
            Ok(())
        })
    })
}

pub fn deregister(id: usize) -> Result<(), Error> {
    HANDLERS.with(|h| {
        let mut h = h.borrow_mut();
        let handler = if h.contains(id) {
            h.remove(id)
        } else {
            return Err(Error::InvalidId);
        };
        POLL.with(|p| {
            debug!("deregistering handler {}", id);
            let poll = p.borrow().expect("outside event loop!");
            poll.deregister(&*handler.borrow())?;
            Ok(())
        })
    })
}

pub fn run_loop_with<F>(init: F) -> Result<(), Error>
    where F: FnOnce() -> Result<(), Error>,
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
                HANDLERS.with(|h| {
                    let ctx_opt = {
                        h.borrow().get(id).map(|ctx| ctx.clone())
                    };
                    ctx_opt.map(|ctx| {
                        ctx.borrow().ready(id, event.readiness());
                    });
                });
            }
        }
        Ok(())
    })?;
    Ok(())
}
