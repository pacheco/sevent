extern crate evmsg;

struct Echo;

impl evmsg::ConnectionHandler for Echo {
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        evmsg::connection_write(id, |wbuf| {
            wbuf.extend(buf.drain(..));
        });
    }

    fn on_disconnect(&mut self, id: usize, err: Option<evmsg::Error>) {
        println!("connection {} disconneted: {:?}", id, err);
    }
}

fn main() {
    evmsg::run_evloop(|| {
        let addr = "127.0.0.1:10000".parse().unwrap();
        let id = evmsg::add_connect(addr, |res| {
            match res {
                Ok(stream) => {
                    println!("connected!");
                    let id = evmsg::add_connection(stream, Echo).unwrap();
                    evmsg::connection_write(id, |wbuf| {
                        wbuf.extend(0..32);
                    });
                }
                Err(err) => {
                    println!("connect error {:?}", err);
                    evmsg::shutdown();
                }
            }
        });
        println!("connect with id {:?}", id);
        Ok(())
    }).unwrap();
}
