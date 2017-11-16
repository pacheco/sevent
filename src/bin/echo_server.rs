extern crate evmsg;
extern crate mio;

struct Echo;

impl evmsg::ConnectionHandler for Echo {
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        evmsg::connection_write(id, |wbuf| {
            wbuf.extend(buf.drain(..));
        });
    }

    fn on_disconnect(&mut self, id: usize, err: Option<evmsg::Error>) {
        println!("connection {} disconnected: {:?}", id, err);
    }
}

fn main() {
    evmsg::run_evloop(|| {
        let addr = "127.0.0.1:10000".parse().unwrap();
        let listener = mio::net::TcpListener::bind(&addr).unwrap();
        let id = evmsg::add_listener(listener, |res| {
            match res {
                Ok((stream, addr)) => {
                    let id = evmsg::add_connection(stream, Echo).unwrap();
                    println!("new connection {} from {:?}", id, addr);
                }
                Err(err) => panic!("{:?}", err),
            }
        }).unwrap();
        println!("listener with id {:?}", id);
        Ok(())
    }).unwrap();
}
