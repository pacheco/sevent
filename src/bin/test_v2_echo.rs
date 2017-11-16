extern crate evmsg;
extern crate mio;
use evmsg::v2;

struct Echo;

impl v2::ConnectionHandler for Echo {
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        v2::connection_write(id, |wbuf| {
            wbuf.extend(buf.drain(..));
        });
    }

    fn on_disconnect(&mut self, id: usize, err: Option<evmsg::Error>) {
        println!("connection {} disconnected: {:?}", id, err);
    }
}

fn main() {
    v2::run_evloop(|| {
        let addr = "127.0.0.1:10000".parse().unwrap();
        let listener = mio::net::TcpListener::bind(&addr).unwrap();
        let id = v2::add_listener(listener, |res| {
            match res {
                Ok((stream, addr)) => {
                    let id = v2::add_connection(stream, Echo).unwrap();
                    println!("new connection {} from {:?}", id, addr);
                }
                Err(err) => panic!("{:?}", err),
            }
        }).unwrap();
        println!("listener with id {:?}", id);
        Ok(())
    }).unwrap();
}
