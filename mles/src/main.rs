extern crate tokio_core;
extern crate futures;
extern crate mles_utils;

use std::collections::HashMap;
use std::rc::Rc;
use std::cell::RefCell;
use std::iter;
use std::env;
use std::io::{Error, ErrorKind};

use tokio_core::net::TcpListener;
use tokio_core::reactor::Core;
use tokio_core::io::{self, Io};

use futures::stream::{self, Stream};
use futures::Future;
use mles_utils::*;

//static mut BUF: Vec<u8> = vec![0,4];

fn main() {
    let addr = env::args().nth(1).unwrap_or("0.0.0.0:8077".to_string());
    let addr = addr.parse().unwrap();
    let mut cnt = 0;

    // Create the event loop and TCP listener we'll accept connections on.
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let socket = TcpListener::bind(&addr, &handle).unwrap();
    println!("Listening on: {}", addr);

    // This is a single-threaded server, so we can just use Rc and RefCell to
    // store the map of all connections we know about.
    let connections = Rc::new(RefCell::new(HashMap::new()));

    let srv = socket.incoming().for_each(move |(stream, addr)| {
        println!("New Connection: {}", addr);
        let (reader, writer) = stream.split();

        // Create a channel for our stream, which other sockets will use to
        // send us messages. Then register our address with the stream to send
        // data to us.
        let (tx, rx) = futures::sync::mpsc::unbounded();
        cnt += 1;
        connections.borrow_mut().insert(cnt, tx);

        // Define here what we do for the actual I/O. That is, read a bunch of
        // frames from the socket and dispatch them while we also write any frames
        // from other sockets.
        let connections_inner = connections.clone();

        // Model the read portion of this socket by mapping an infinite
        // iterator to each frame off the socket. This "loop" is then
        // terminated with an error once we hit EOF on the socket.
        let iter = stream::iter(iter::repeat(()).map(Ok::<(), Error>));
        let socket_reader = iter.fold(reader, move |reader, _| {
            // Read a off header the socket, failing if we're at EOF
            let frame = io::read_exact(reader, vec![0;4]);
            let frame = frame.and_then(move |(reader, payload)| {
                if payload.len() == 0 {
                    Err(Error::new(ErrorKind::BrokenPipe, "broken pipe"))
                } else {
                    if read_hdr_type(payload.as_slice()) != 'M' as u32 {
                        return Err(Error::new(ErrorKind::BrokenPipe, "incorrect header"));
                    }
                    let hdr_len = read_hdr_len(payload.as_slice());
                    if 0 == hdr_len {
                        return Err(Error::new(ErrorKind::BrokenPipe, "incorrect header len"));
                    }
                    Ok((reader, hdr_len))
                }
            });

            let frame = frame.and_then(move |(reader, hdr_len)| {
                println!("Hdrlen {}", hdr_len);
                let tframe = io::read_exact(reader, vec![0;hdr_len]);
                let tframe = tframe.and_then(move |(reader, payload)| {
                    let decoded_message = message_decode(payload.as_slice());
                    println!("Message {:?}", decoded_message);
                    Ok((reader, message_encode(&decoded_message)))
                });
                tframe
            });

            println!("Next to send ");
            // Send frame to all other connected clients
            let connections = connections_inner.clone();
            frame.map(move |(reader, message)| {
                println!("Message {}: {:?}", cnt, message);
                let mut conns = connections.borrow_mut();
                let iter = conns.iter_mut()
                .filter(|&(&k, _)| k != cnt)
                .map(|(_, v)| v);
                for tx in iter {
                    println!("Sending {}: {:?}", cnt, message);
                    tx.send(message.clone()).unwrap();
                }
                reader
            })
        });

        // Whenever we receive a string on the Receiver, we write it to
        // `WriteHalf<TcpStream>`.
        let socket_writer = rx.fold(writer, |writer, msg| {
            let amt = io::write_all(writer, msg);
            let amt = amt.map(|(writer, _)| writer);
            amt.map_err(|_| ())
        });

        // Now that we've got futures representing each half of the socket, we
        // use the `select` combinator to wait for either half to be done to
        // tear down the other. Then we spawn off the result.
        let connections = connections.clone();
        let socket_reader = socket_reader.map_err(|_| ());
        let connection = socket_reader.map(|_| ()).select(socket_writer.map(|_| ()));
        handle.spawn(connection.then(move |_| {
            connections.borrow_mut().remove(&cnt);
            println!("Connection {} closed.", addr);
            Ok(())
        }));

        Ok(())
    });

    // execute server
    core.run(srv).unwrap();
}
