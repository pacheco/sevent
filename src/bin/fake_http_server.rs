extern crate evmsg;
extern crate mio;

use mio::net::TcpListener;

const RESPONSE: &'static str = "HTTP/1.1 200 OK\r
Content-Length: 14\r
\r
Hello World\r
\r";

fn main() {
    let addr = "127.0.0.1:10000".parse().unwrap();

    evmsg::run_evloop(|| {
        let l = TcpListener::bind(&addr)?;
        evmsg::add_listener(l, |res: Result<_,_>| {
            let (stream, addr) = res.unwrap();

            let id = evmsg::add_connection(stream, evmsg::ConnectionHandlerClosures {
                on_read: |_id, buf| {
                    buf.drain(..); // just discard the data on read
                },
                on_disconnect: |id, err| {
                    println!("client {} disconnected: {:?}", id, err);
                },
                on_write_finished: |id| {
                    // write to client
                    evmsg::connection_write(id, |wbuf| {
                        wbuf.extend_from_slice(RESPONSE.as_bytes());
                    })
                },
            }).unwrap();

            println!("client connection {} from {:?}", id, addr);

            // write to client
            evmsg::connection_write(id, |wbuf| {
                wbuf.extend_from_slice(RESPONSE.as_bytes());
            })
        }).unwrap();
        Ok(())
    }).unwrap();
}
