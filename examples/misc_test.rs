extern crate sevent;
extern crate mio_more;

use std::time::Duration;
use std::thread;

use mio_more::channel;

struct TimeoutCount(u64);

impl sevent::TimeoutHandler for TimeoutCount {
    fn on_timeout(mut self: Box<TimeoutCount>) {
        if self.0 == 0 {
            println!("count done!");
            sevent::shutdown();
        } else {
            println!("counting down: {}", self.0);
            self.0 -= 1;
            sevent::set_timeout(Duration::from_secs(1), *self).unwrap();
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

    sevent::run_evloop(|| {
        sevent::on_current_tick(|| {
            println!("end of first tick!");
            sevent::on_next_tick(|| println!("end of second tick!"));
            sevent::on_current_tick(|| println!("end of first tick!"));
        });
        sevent::set_timeout(Duration::from_secs(1), || {
            println!("timed out!");
        }).unwrap();
        sevent::set_timeout(Duration::from_secs(0), TimeoutCount(10)).unwrap();
        sevent::add_chan(rx, |id, recv| {
            if let Some(msg) = recv {
                println!("got {} from chan {}", msg, id);
            }
        }).unwrap();
        Ok(())
    }).unwrap();
}
