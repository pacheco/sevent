extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate mio;
extern crate sevent;

use std::net::SocketAddr;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashMap;

use mio::net::TcpListener;
use mio::net::TcpStream;

use sevent::circular_buf::CircularBuffer;

#[derive(Serialize,Deserialize)]
enum Request {
    Put { key: String, val: String },
    Get { key: String },
}

#[derive(Serialize,Deserialize)]
enum Reply {
    Get { val: Option<String> },
    Put { old_val: Option<String> },
}

#[derive(Clone)]
struct Server {
    kv: Rc<RefCell<HashMap<String, String>>>,
}

impl sevent::AcceptHandler for Server {
    fn on_accept(&mut self, res: Result<(TcpStream, SocketAddr), sevent::Error>) {
        let (stream, addr) = res.unwrap();
        println!("new connection from: {:?}", addr);
        sevent::add_connection(stream, self.clone()).unwrap();
    }
}

impl sevent::ConnectionHandler for Server {
    fn on_read(&mut self, conn_id: usize, buf: &mut CircularBuffer) {
        for msg in buf.drain_frames_bincode() {
            let reply = match msg.unwrap() {
                Request::Get { key } => {
                    let val = self.kv.borrow_mut().get(&key).cloned();
                    Reply::Get { val }
                }
                Request::Put { key, val } => {
                    let old_val = self.kv.borrow_mut().insert(key, val);
                    Reply::Put { old_val }
                }
            };
            sevent::connection_write(conn_id, |wbuf| {
                wbuf.put_frame_bincode(&reply).unwrap();
            }).unwrap()
        }
    }
}

fn main() {
    sevent::run_evloop(|| {
        let listener = TcpListener::bind(&"127.0.0.1:10000".parse().unwrap()).unwrap();
        sevent::add_listener(listener, Server { kv: Rc::default() }).unwrap();
        Ok(())
    }).unwrap();
}
