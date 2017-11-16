use std;
use std::cell::RefCell;
use std::rc::Rc;

use mio::Poll;
use mio::PollOpt;
use mio::Ready;
use mio::Token;
use mio::event::Evented;

use ::EventHandler;

// 

// EventedFn --------------------------------

pub struct EventedFn<E: Evented, F: Fn(&mut E, usize, Ready)>(E, F);

pub fn evented_with<E, F>(evented: E, f: F) -> Rc<RefCell<EventedFn<E, F>>>
    where E: Evented,
          F: Fn(&mut E, usize, Ready),
{
    EventedFn::new_wrapped(evented, f)
}

impl<E, F> EventedFn<E, F>
    where E: Evented,
          F: Fn(&mut E, usize, Ready),
{
    pub fn new_wrapped(evented: E, f: F) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(EventedFn(evented, f)))
    }
}

impl<E, F> Evented for EventedFn<E, F>
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

impl<E, F> EventHandler for EventedFn<E, F>
    where E: Evented,
          F: Fn(&mut E, usize, Ready),
{
    fn ready(&self, self_id: usize, ready: Ready) {
        let e = &self.0 as *const E as *mut E;
        let f = &self.1;
        unsafe { f(&mut *e, self_id, ready) }
    }
}
