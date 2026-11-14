//! A stand-in SOCKS5 proxy for the tests (`tests/tls.rs`: the launcher's own HTTPS client;
//! `tests/egress.rs`: connections the relay passes through for the game). Included with
//! `#[path]`, not a test target of its own.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// A SOCKS5 proxy that records what it was asked to connect to and then connects to
/// `upstream` whatever the name was.
pub async fn socks5_proxy(upstream: SocketAddr, refuse: bool) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = asked.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else { break };
            let log = log.clone();
            tokio::spawn(async move {
                let mut greeting = [0u8; 3];
                client.read_exact(&mut greeting).await.unwrap();
                assert_eq!(greeting, [5, 1, 0], "version 5, one method, no authentication");
                client.write_all(&[5, 0]).await.unwrap();
                let mut head = [0u8; 5];
                client.read_exact(&mut head).await.unwrap();
                assert_eq!(&head[..4], &[5, 1, 0, 3], "CONNECT to a domain name: the proxy resolves");
                let mut name = vec![0u8; head[4] as usize + 2];
                client.read_exact(&mut name).await.unwrap();
                let port = u16::from_be_bytes([name[name.len() - 2], name[name.len() - 1]]);
                name.truncate(name.len() - 2);
                log.lock().unwrap().push(format!("{}:{port}", String::from_utf8(name).unwrap()));
                if refuse {
                    client.write_all(&[5, 2, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
                    return;
                }
                let mut server = tokio::net::TcpStream::connect(upstream).await.unwrap();
                client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0x1f, 0x90]).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
            });
        }
    });
    (addr, asked)
}
