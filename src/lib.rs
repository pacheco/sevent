#[macro_use]
extern crate log;
extern crate mio;
extern crate lazycell;
extern crate slab;

pub mod errors;
pub use errors::Error;

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
        let handler = h.get(id).ok_or(Error::InvalidHandlerId)?;
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
            return Err(Error::InvalidHandlerId);
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
                    let ctx_opt = Option::cloned(h.borrow_mut().get(id));
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

// --------------------------------

pub struct EventFn<E: Evented, F: Fn(&mut E, usize, Ready)>(E, F);

pub fn handler<E, F>(evented: E, f: F) -> Rc<RefCell<EventFn<E, F>>>
    where E: Evented,
          F: Fn(&mut E, usize, Ready),
{
    EventFn::new_wrapped(evented, f)
}

impl<E, F> EventFn<E, F>
    where E: Evented,
          F: Fn(&mut E, usize, Ready),
{
    pub fn new_wrapped(evented: E, f: F) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(EventFn(evented, f)))
    }
}

impl<E, F> Evented for EventFn<E, F>
    where E: Evented,
          F: Fn(&mut E, usize, Ready),
{
    fn register(&self, poll: &Poll, id: Token, ready: Ready, opt: PollOpt) -> Result<(), std::io::Error> {
        poll.register(&self.0, id, ready, opt)
    }

    fn reregister(&self, poll: &Poll, id: Token, ready: Ready, opt: PollOpt) -> Result<(), std::io::Error> {
        poll.reregister(&self.0, id, ready, opt)
    }

    fn deregister(&self, poll: &Poll) -> Result<(), std::io::Error> {
        poll.deregister(&self.0)
    }
}

impl<E, F> EventHandler for EventFn<E, F>
    where E: Evented,
          F: Fn(&mut E, usize, Ready),
{
    fn ready(&self, self_id: usize, ready: Ready) {
        let e = &self.0 as *const E as *mut E;
        let f = &self.1;
        unsafe { f(&mut *e, self_id, ready) }
    }
}
