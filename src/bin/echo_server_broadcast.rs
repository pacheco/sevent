extern crate sevent;
extern crate mio;

use mio::net::TcpStream;

use std::time::SystemTime;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashSet;

use sevent::ext::VecExt;

struct Echo {
    connections: Rc<RefCell<HashSet<usize>>>,
}

impl sevent::ConnectionHandler for Echo {
    fn on_read(&mut self, _id: usize, buf: &mut Vec<u8>) {
        let connections = self.connections.borrow();
        for msg in buf.drain_frames_bincode() {
            let msg: (u64, SystemTime) = msg.unwrap();
            for id in connections.iter() {
                sevent::connection_write(*id, |wbuf| {
                    wbuf.put_frame_bincode(&msg).unwrap();
                }).unwrap();
            }
        }
    }

    fn on_disconnect(&mut self, id: usize, err: Option<sevent::Error>) {
        self.connections.borrow_mut().remove(&id);
        println!("connection {} disconnected: {:?}", id, err);
    }
}

fn main() {
    sevent::run_evloop(|| {
        let addr = "127.0.0.1:10000".parse().unwrap();

        let connections: Rc<RefCell<HashSet<usize>>> = Rc::default();

        let listener = mio::net::TcpListener::bind(&addr).unwrap();

        let id = sevent::add_listener(listener, move |res: Result<(TcpStream, _),_>| {
            match res {
                Ok((stream, addr)) => {
                    stream.set_nodelay(true).unwrap();
                    let id = sevent::add_connection(stream, Echo {
                        connections: connections.clone(),
                    }).unwrap();
                    connections.borrow_mut().insert(id);
                    println!("new connection {} from {:?}", id, addr);
                }
                Err(err) => panic!("{:?}", err),
            }
        }).unwrap();
        println!("listener with id {:?}", id);
        Ok(())
    }).unwrap();
}
