extern crate histogram;
extern crate sevent;
extern crate rand;
extern crate mio;

use histogram::Histogram;

use sevent::circular_buf::CircularBuffer;

use std::time::Duration;
use std::time::SystemTime;
use std::collections::HashSet;
use std::rc::Rc;
use std::cell::RefCell;

use mio::net::TcpStream;


struct Echo {
    stats: Rc<RefCell<Histogram>>,
    pending: Rc<RefCell<HashSet<u64>>>,
}

pub fn duration_as_usecs(d: Duration) -> u64 {
    d.as_secs()*1000000 + d.subsec_nanos() as u64/1000
}

impl sevent::ConnectionHandler for Echo {
    fn on_read(&mut self, id: usize, buf: &mut CircularBuffer) {
        let mut pending = self.pending.borrow_mut();
        for msg in buf.drain_frames_bincode() {
            let (msg_id, send_time): (u64, SystemTime) = msg.unwrap();
            if pending.remove(&msg_id) {
                let now = SystemTime::now();
                let lat = now.duration_since(send_time).unwrap();
                let mut stats = self.stats.borrow_mut();
                stats.increment(duration_as_usecs(lat)).unwrap();
                // send another msg
                sevent::connection_write(id, |wbuf| {
                    let msg: (u64, _) = (rand::random(), SystemTime::now());
                    pending.insert(msg.0);
                    wbuf.put_frame_bincode(&msg).unwrap();
                }).unwrap();
            }
        }
    }

    fn on_disconnect(&mut self, id: usize, err: Option<sevent::Error>) {
        println!("connection {} disconneted: {:?}", id, err);
    }
}

fn print_stats(hist: &Histogram) {
    if hist.entries() > 0 {
        println!("tput: {} op/sec\tavg_lat: {} usec\tmax_lat: {}\tstd_dev: {}\n\
                  \tmedian: {}\t95th: {}\t99th: {}\t99.9th: {}",
                 hist.entries(),
                 hist.mean().unwrap(),
                 hist.maximum().unwrap(),
                 hist.stddev().unwrap(),
                 hist.percentile(50.0).unwrap(),
                 hist.percentile(95.0).unwrap(),
                 hist.percentile(99.0).unwrap(),
                 hist.percentile(99.9).unwrap());
    } else {
        println!("tput: 0 op/sec");
    }
}

struct PrintStats {
    stats: Rc<RefCell<Histogram>>,
}

impl sevent::TimeoutHandler for PrintStats {
    fn on_timeout(self: Box<Self>) {
        {
            let mut stats = self.stats.borrow_mut();
            print_stats(&*stats);
            stats.clear();
        }
        sevent::set_timeout(Duration::from_secs(1), *self).unwrap();
    }
}


fn main() {
    // the first cmdline arg is the number of concurrent outstanding
    // requests the client is allowed to have.
    sevent::run_evloop(|| {

        let addr = "127.0.0.1:10000".parse().unwrap();

        let outstanding: usize = std::env::args().nth(1).unwrap().parse().unwrap();

        let pending = Rc::new(RefCell::new(HashSet::new()));

        // print statistics
        let stats = Rc::new(RefCell::new(Histogram::new()));
        sevent::set_timeout(Duration::from_secs(1), PrintStats { stats: stats.clone() })?;

        let echo = Echo {
            pending: pending.clone(),
            stats,
        };

        sevent::add_connect(addr, move |addr, res: Result<TcpStream, _>| {
            match res {
                Ok(stream) => {
                    stream.set_nodelay(true).unwrap();
                    println!("connected to {:?}!", addr);
                    let conn_id = sevent::add_connection(stream, echo).unwrap();
                    for _ in 0..outstanding {
                        sevent::connection_write(conn_id, |wbuf| {
                            let id: u64 = rand::random();
                            let msg = (id, SystemTime::now());
                            pending.borrow_mut().insert(id);
                            wbuf.put_frame_bincode(&msg).unwrap();
                        }).unwrap();
                    }
                }
                Err(err) => {
                    println!("error connecting to {:?}: {:?}", addr, err);
                    sevent::shutdown();
                }
            }
        }).unwrap();
        Ok(())
    }).unwrap();
}
