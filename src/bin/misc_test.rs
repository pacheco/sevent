extern crate evmsg;
extern crate mio_more;

use std::time::Duration;
use std::thread;

use mio_more::channel;

struct TimeoutCount(u64);

impl evmsg::TimeoutHandler for TimeoutCount {
    fn timeout(mut self: Box<TimeoutCount>) {
        if self.0 == 0 {
            println!("count done!");
        } else {
            println!("counting down: {}", self.0);
            self.0 -= 1;
            evmsg::set_timeout(Duration::from_secs(1), *self).unwrap();
        }
    }
}

fn main() {
    let (tx,rx) = channel::channel();

    thread::spawn(move || {
        loop {
            tx.send("hey!".to_string()).unwrap();
            thread::sleep(Duration::from_secs(1));
        }
    });

    evmsg::run_evloop(|| {
        evmsg::set_timeout(Duration::from_secs(1), || {
            println!("timed out!");
        }).unwrap();
        evmsg::set_timeout(Duration::from_secs(0), TimeoutCount(10)).unwrap();
        evmsg::add_chan(rx, |id, recv| {
            if let Some(msg) = recv {
                println!("got {} from chan {}", msg, id);
            }
        }).unwrap();
        Ok(())
    }).unwrap();
}
