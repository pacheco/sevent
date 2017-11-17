extern crate sevent;
use sevent::ext::VecExt;

struct Echo;

impl sevent::ConnectionHandler for Echo {
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        let msgs = buf.drain_frames_bincode::<String>();
        sevent::connection_write(id, |wbuf| {
            for msg in msgs {
                wbuf.put_frame_bincode(&msg.unwrap()).unwrap();
            }
        });
    }

    fn on_disconnect(&mut self, id: usize, err: Option<sevent::Error>) {
        println!("connection {} disconneted: {:?}", id, err);
    }
}

fn main() {
    sevent::run_evloop(|| {
        let addr = "127.0.0.1:10000".parse().unwrap();
        let id = sevent::add_connect(addr, |addr, res| {
            match res {
                Ok(stream) => {
                    println!("connected to {:?}!", addr);
                    let id = sevent::add_connection(stream, Echo).unwrap();
                    sevent::connection_write(id, |wbuf| {
                        wbuf.put_frame_bincode(&"hey joe!".to_string()).unwrap();
                    });
                }
                Err(err) => {
                    println!("connect error {:?}", err);
                    sevent::shutdown();
                }
            }
        });
        println!("connect with id {:?}", id);
        Ok(())
    }).unwrap();
}
