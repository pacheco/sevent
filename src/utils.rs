use std::net::SocketAddr;

/// Simple `TimeoutHandler` for reconnects
pub struct Reconnect<H: ::ConnectHandler> {
    pub addr: SocketAddr,
    pub handler: H,
}

impl<H: 'static + ::ConnectHandler> ::TimeoutHandler for Reconnect<H> {
    fn on_timeout(self: Box<Self>) {
        ::add_connect(self.addr, self.handler).unwrap();
    }
}
