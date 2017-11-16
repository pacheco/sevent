extern crate evmsg;
use evmsg::v2;

struct Echo;

impl v2::ConnectionHandler for Echo {
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        v2::connection_write(id, |wbuf| {
            wbuf.extend(buf.drain(..));
        });
    }

    fn on_disconnect(&mut self, id: usize, err: Option<evmsg::Error>) {
        println!("connection {} disconneted: {:?}", id, err);
    }
}

fn main() {
    v2::run_evloop(|| {
        let addr = "127.0.0.1:10000".parse().unwrap();
        let id = v2::add_connect(addr, |res| {
            match res {
                Ok(stream) => {
                    println!("connected!");
                    let id = v2::add_connection(stream, Echo).unwrap();
                    v2::connection_write(id, |wbuf| {
                        wbuf.extend(0..32);
                    });
                }
                Err(err) => {
                    println!("connect error {:?}", err);
                    v2::shutdown();
                }
            }
        });
        println!("connect with id {:?}", id);
        Ok(())
    }).unwrap();
}
