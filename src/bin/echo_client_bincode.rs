extern crate evmsg;
use evmsg::ext::VecExt;

struct Echo;

impl evmsg::ConnectionHandler for Echo {
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        let msgs = buf.drain_frames_bincode::<String>();
        evmsg::connection_write(id, |wbuf| {
            for msg in msgs {
                wbuf.put_frame_bincode(&msg.unwrap()).unwrap();
            }
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
                        wbuf.put_frame_bincode(&"hey joe!".to_string()).unwrap();
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
