extern crate sevent;

struct Echo;

impl sevent::ConnectionHandler for Echo {
    fn on_read(&mut self, id: usize, buf: &mut Vec<u8>) {
        sevent::connection_write(id, |wbuf| {
            wbuf.extend(buf.drain(..));
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
                        wbuf.extend(0..32);
                    });
                }
                Err(err) => {
                    println!("error connecting to {:?}: {:?}", addr, err);
                    sevent::shutdown();
                }
            }
        });
        println!("connect with id {:?}", id);
        Ok(())
    }).unwrap();
}
