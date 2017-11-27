extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate sevent;
extern crate mio_more;
extern crate lazycell;

use std::io;
use std::io::BufRead;
use std::io::Write;
use std::thread;
use std::sync::mpsc;

use lazycell::LazyCell;

use mio_more::channel;

use sevent::ext::VecExt;

#[derive(Serialize,Deserialize)]
enum Request {
    Put { key: String, val: String },
    Get { key: String },
}

#[derive(Debug,Serialize,Deserialize)]
enum Reply {
    Get { key: Option<String> },
    Put { old_val: Option<String> },
}

struct ClientConn {
    reply_tx: LazyCell<mpsc::Sender<Reply>>,
}

impl ClientConn {
    fn new() -> Self {
        ClientConn { reply_tx: LazyCell::new() }
    }
}

impl sevent::ConnectionHandler for ClientConn {
    fn on_add(&mut self, conn_id: usize) {
        // we'll read commands from stdin from another thread and pass
        // them to the event loop using a channel
        let (req_tx, req_rx) = channel::channel();
        let (reply_tx, reply_rx) = mpsc::channel();

        self.reply_tx.fill(reply_tx).ok();

        // read cmds from stdin
        thread::spawn(move || {
            let bufin = io::BufReader::new(io::stdin());
            print!("> ");
            io::stdout().flush().unwrap();
            for line in bufin.lines() {
                if let Ok(line) = line {
                    let parts: Vec<_> = line.split_whitespace().collect();
                    match parts[0] {
                        "get" => {
                            let key = parts[1].to_string();
                            req_tx.send(Request::Get { key }).unwrap();
                        }
                        "put" => {
                            let key = parts[1].to_string();
                            let val = parts[2].to_string();
                            req_tx.send(Request::Put { key, val }).unwrap();
                        }
                        _ => {
                            println!("invalid command");
                        }
                    }
                    let reply = reply_rx.recv().unwrap();
                    println!("result: {:?}", reply);
                    print!("> ");
                    io::stdout().flush().unwrap();
                }
            }
        });

        // send cmds received through the req channel
        sevent::add_chan(req_rx, move |_chan_id, opt_msg| {
            if let Some(msg) = opt_msg {
                sevent::connection_write(conn_id, |wbuf| {
                    wbuf.put_frame_bincode(&msg).unwrap();
                }).unwrap();
            }
        }).unwrap();
    }

    fn on_read(&mut self, _conn_id: usize, buf: &mut Vec<u8>) {
        for msg in buf.drain_frames_bincode() {
            self.reply_tx.borrow().unwrap().send(msg.unwrap()).unwrap();
        }
    }
}

fn main() {
    sevent::run_evloop(|| {
        sevent::add_connect("127.0.0.1:10000".parse().unwrap(), |_id, res| {
            match res {
                Ok(stream) => {
                    sevent::add_connection(stream, ClientConn::new()).unwrap();
                }
                Err(err) => panic!(err),
            }
        }).unwrap();

        Ok(())
    }).unwrap();
}
