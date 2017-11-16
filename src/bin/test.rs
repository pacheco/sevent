extern crate evmsg;
extern crate mio;
extern crate bytes;

use evmsg::handler;

use std::cell::RefCell;
use std::io::ErrorKind::WouldBlock;
use std::io::Read;
use std::io::Write;

use mio::*;

fn main() {
    evmsg::run_loop_with(|| {
        let l = mio::net::TcpListener::bind(&"127.0.0.1:10000".parse().unwrap())?;
        let server = handler::evented_with(l, |l, _self_id, _ready| {
            match l.accept() {
                Err(ref err) if err.kind() == WouldBlock => return,
                Err(err) => panic!(err),
                Ok((stream, addr)) => {
                    println!("new connection from {:?}", addr);
                    let data = RefCell::new(Vec::<u8>::new());
                    let echo = handler::evented_with(stream, move |s, id, ready| {
                        // println!("stream {} event: {:?}", id, ready);
                        if ready.is_readable() {
                            let mut data = data.borrow_mut();
                            assert!(data.is_empty());
                            // read
                            loop {
                                // make space for reading 8k
                                let max_read = 8*1024;
                                let orig_len = data.len();
                                data.reserve(max_read);
                                unsafe { data.set_len(orig_len + max_read) };
                                match s.read(&mut data[orig_len..]) {
                                    Ok(0) => {
                                        println!("stream {} closed by remote", id);
                                        evmsg::deregister(id).unwrap();
                                        return;
                                    }
                                    Ok(n) => {
                                        // println!("stream {} read {} bytes", id, n);
                                        unsafe { data.set_len(orig_len + n) };
                                    }
                                    Err(ref err) if err.kind() == WouldBlock => {
                                        // println!("would block");
                                        unsafe { data.set_len(orig_len) };
                                        break;
                                    }
                                    Err(err) => {
                                        println!("stream {} error: {:?}", id, err);
                                        evmsg::deregister(id).unwrap();
                                        return;
                                    }
                                }
                            }
                            // write
                            while !data.is_empty() {
                                match s.write(&data[..]) {
                                    Ok(n) => {
                                        // println!("stream {} wrote {} bytes", id, n);
                                        data.drain(..n);
                                    }
                                    Err(ref err) if err.kind() == WouldBlock => {
                                        break;
                                    }
                                    Err(err) => {
                                        println!("stream {} error: {:?}", id, err);
                                        evmsg::deregister(id).unwrap();
                                        return;
                                    }
                                }
                            }
                            if data.is_empty() {
                                evmsg::reregister(id, Ready::readable(), PollOpt::edge()).unwrap();
                            } else {
                                evmsg::reregister(id,
                                                  Ready::readable() | Ready::writable(),
                                                  PollOpt::edge())
                                    .unwrap();
                            }
                        }
                    });
                    evmsg::register(echo, Ready::readable(), PollOpt::edge()).unwrap();
                }
            }
        });
        evmsg::register(server, Ready::readable(), PollOpt::level())?;
        Ok(())
    }).unwrap();
}
