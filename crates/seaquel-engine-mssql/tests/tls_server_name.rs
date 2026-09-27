//! `ConnectConfig::tls_server_name` (phase 5a, Decision 6 row 6): the socket
//! goes to `host` and `port`, and TLS checks the certificate against
//! `tls_server_name`. Through an SSH tunnel `host` is `127.0.0.1` and the
//! name is the server's own.
//!
//! A fake SQL Server on a local port answers PRELOGIN with encryption on and
//! reads the TLS ClientHello tiberius sends inside the next TDS packet. Its
//! SNI is the name the certificate will be checked against (rustls sends
//! none for an IP address).

use seaquel_engine::ConnectConfig;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_packet(s: &mut TcpStream) -> Option<Vec<u8>> {
    let mut header = [0u8; 8];
    s.read_exact(&mut header).await.ok()?;
    let len = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut body = vec![0u8; len.checked_sub(8)?];
    s.read_exact(&mut body).await.ok()?;
    Some(body)
}

/// The SNI host name in a TLS ClientHello record, if any.
fn sni(record: &[u8]) -> Option<String> {
    let hello = record.get(5..)?; // record header
    let mut i = 4 + 2 + 32; // handshake header, version, random
    i += 1 + *hello.get(i)? as usize; // session id
    i += 2 + u16::from_be_bytes([*hello.get(i)?, *hello.get(i + 1)?]) as usize; // ciphers
    i += 1 + *hello.get(i)? as usize; // compression
    let end = i + 2 + u16::from_be_bytes([*hello.get(i)?, *hello.get(i + 1)?]) as usize;
    i += 2;
    while i + 4 <= end {
        let ty = u16::from_be_bytes([hello[i], hello[i + 1]]);
        let len = u16::from_be_bytes([hello[i + 2], hello[i + 3]]) as usize;
        let data = hello.get(i + 4..i + 4 + len)?;
        if ty == 0 {
            // list length (2), name type (1), name length (2), name
            let n = u16::from_be_bytes([data[3], data[4]]) as usize;
            return Some(String::from_utf8_lossy(&data[5..5 + n]).into_owned());
        }
        i += 4 + len;
    }
    None
}

/// Connects with `config` to a fake server on `listener` and returns the
/// ClientHello's SNI (`None` when it has none).
async fn sni_of(listener: TcpListener, config: ConnectConfig) -> Option<String> {
    let server = async move {
        let (mut s, _) = listener.accept().await.unwrap();
        read_packet(&mut s).await.unwrap(); // PRELOGIN
                                            // PRELOGIN response: VERSION (6 bytes) and ENCRYPTION = ON.
        let payload = [
            0x00, 0, 11, 0, 6, 0x01, 0, 17, 0, 1, 0xFF, 16, 0, 0, 0, 0, 0, 0x01,
        ];
        let mut packet = vec![0x04, 0x01, 0, (payload.len() + 8) as u8, 0, 0, 1, 0];
        packet.extend_from_slice(&payload);
        s.write_all(&packet).await.unwrap();
        let hello = read_packet(&mut s).await.unwrap();
        // Dropping the socket ends the client's handshake.
        sni(&hello)
    };
    let engine = seaquel_engine_mssql::engine();
    let both = async { tokio::join!(server, engine.open(&config)) };
    let (name, _) = tokio::time::timeout(std::time::Duration::from_secs(10), both)
        .await
        .expect("the client never reached the fake server");
    name
}

fn config(port: u16, tls_server_name: Option<&str>) -> ConnectConfig {
    serde_json::from_value(json!({
        "driver": "mssql", "host": "127.0.0.1", "port": port,
        "username": "sa", "password": "pw", "encrypt": true, "trust_cert": false,
        "tls_server_name": tls_server_name,
    }))
    .unwrap()
}

#[tokio::test]
async fn the_tls_name_is_tls_server_name_while_the_socket_goes_to_host() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let name = sni_of(listener, config(port, Some("sql.internal"))).await;
    assert_eq!(name.as_deref(), Some("sql.internal"));
}

#[tokio::test]
async fn without_it_the_tls_name_is_host() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // `127.0.0.1` is an IP address: no SNI, and the certificate would be
    // checked against the address.
    assert_eq!(sni_of(listener, config(port, None)).await, None);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut c = config(port, None);
    c.host = Some("localhost".into());
    assert_eq!(sni_of(listener, c).await.as_deref(), Some("localhost"));
}

/// A bracketed IPv6 host (`[::1]`) is dialled as the bare address.
#[tokio::test]
async fn a_bracketed_ipv6_host_is_dialled() {
    let Ok(listener) = TcpListener::bind("[::1]:0").await else {
        eprintln!("skipping: no IPv6 loopback");
        return;
    };
    let port = listener.local_addr().unwrap().port();
    let mut c = config(port, None);
    c.host = Some("[::1]".into());
    // The fake server got the PRELOGIN and a ClientHello (an IP address
    // sends no SNI), so the socket reached it.
    assert_eq!(sni_of(listener, c).await, None);
}
