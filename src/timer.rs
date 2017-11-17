// TODO: unboxed closures would make sense here but are unstable

pub trait TimeoutHandler {
    fn on_timeout(self: Box<Self>);
}

impl<F> TimeoutHandler for F
    where F: FnOnce()
{
    fn on_timeout(self: Box<Self>) {
        self() 
    }
}
