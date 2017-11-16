extern crate evmsg;
extern crate mio_more;

use std::thread;

use mio_more::channel;

fn main() {
    let (tx,rx) = channel::channel();

    thread::spawn(move || {
        loop {
            tx.send("hey!".to_string()).unwrap();
            thread::sleep(std::time::Duration::from_secs(1));
        }
    });

    evmsg::run_evloop(|| {
        evmsg::add_chan(rx, |id, recv| {
            if let Some(msg) = recv {
                println!("got {} from chan {}", msg, id);
            }
        }).unwrap();
        Ok(())
    }).unwrap();
}
