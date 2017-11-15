extern crate evmsg;
extern crate mio;

use std::cell::RefCell;
use std::io::ErrorKind::WouldBlock;
use std::io::Read;
use std::io::Write;

use mio::*;

fn main() {
    evmsg::run_loop_with(|| {
        let l = mio::net::TcpListener::bind(&"127.0.0.1:10000".parse().unwrap())?;
        let server = evmsg::handler(l, |l, _self_id, _ready| {
            match l.accept() {
                Err(err) => {
                    if let WouldBlock = err.kind() {
                        return;
                    } else {
                        panic!(err);
                    }
                }
                Ok((stream, addr)) => {
                    println!("new connection from {:?}", addr);
                    let data = RefCell::new(Vec::<u8>::new());
                    let echo = evmsg::handler(stream, move |s, id, ready| {
                        if ready.is_readable() {
                            let mut data = data.borrow_mut();
                            // read
                            let mut buf = [0u8;8*1024];
                            loop {
                                match s.read(&mut buf[..]) {
                                    Ok(0) => {
                                        println!("stream {} closed by remote", id);
                                        evmsg::deregister(id).unwrap();
                                        return;
                                    }
                                    Ok(n) => {
                                        println!("stream {} read {} bytes", id, n);
                                        data.extend(&buf[..n]);
                                    }
                                    Err(err) => {
                                        if let WouldBlock = err.kind() {
                                            break;
                                        } else {
                                            println!("stream {} error: {:?}", id, err);
                                            evmsg::deregister(id).unwrap();
                                            return;
                                        }
                                    }
                                }
                            }
                            // write
                            while !data.is_empty() {
                                match s.write(&data[..]) {
                                    Ok(n) => {
                                        println!("stream {} wrote {} bytes", id, n);
                                        data.drain(..n);
                                    }
                                    Err(err) => {
                                        if let WouldBlock = err.kind() {
                                            break;
                                        } else {
                                            println!("stream {} error: {:?}", id, err);
                                            evmsg::deregister(id).unwrap();
                                            return;
                                        }
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
                        println!("stream {} event: {:?}", id, ready);
                    });
                    evmsg::register(echo, Ready::readable(), PollOpt::edge()).unwrap();
                }
            }
        });
        evmsg::register(server, Ready::readable(), PollOpt::level())?;
        Ok(())
    }).unwrap();
}
