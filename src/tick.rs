// TODO: unboxed closures would make sense here but are unstable

pub trait TickHandler {
    fn on_tick(self: Box<Self>);
}

impl<F> TickHandler for F
    where F: FnOnce()
{
    fn on_tick(self: Box<Self>) {
        self()
    }
}
