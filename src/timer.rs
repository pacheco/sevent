pub trait TimeoutHandler {
    fn timeout(self: Box<Self>);
}

impl<F> TimeoutHandler for F
    where F: FnOnce()
{
    fn timeout(self: Box<Self>) {
        self() 
    }
}
