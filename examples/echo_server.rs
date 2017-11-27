extern crate sevent;
extern crate mio;

use mio::net::TcpStream;

struct Echo;

impl sevent::ConnectionHandler for Echo {
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        sevent::connection_write(id, |wbuf| {
            wbuf.extend(buf.drain(..));
        }).unwrap();
    }

    fn on_disconnect(&mut self, id: usize, err: Option<sevent::Error>) {
        println!("connection {} disconnected: {:?}", id, err);
    }
}

fn main() {
    sevent::run_evloop(|| {
        let addr = "127.0.0.1:10000".parse().unwrap();
        let listener = mio::net::TcpListener::bind(&addr).unwrap();
        let id = sevent::add_listener(listener, |res: Result<(TcpStream, _),_>| {
            match res {
                Ok((stream, addr)) => {
                    stream.set_nodelay(true).unwrap();
                    let id = sevent::add_connection(stream, Echo).unwrap();
                    println!("new connection {} from {:?}", id, addr);
                }
                Err(err) => panic!("{:?}", err),
            }
        }).unwrap();
        println!("listener with id {:?}", id);
        Ok(())
    }).unwrap();
}
