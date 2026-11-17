//! Dependency-free private-loopback probe; never prints response bodies or credentials.
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;
fn main() {
    let result = (|| -> std::io::Result<bool> {
        let port = std::env::var("CBGPU_HEALTH_PORT")
            .unwrap_or("3546".into())
            .parse::<u16>()
            .map_err(|_| std::io::Error::other("Invalid port"))?;
        if port == 0 {
            return Ok(false);
        }
        let mut stream = TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_secs(2),
        )?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        let mut status = [0u8; 128];
        let count = stream.read(&mut status)?;
        Ok(status[..count].starts_with(b"HTTP/1.1 200 ")
            || status[..count].starts_with(b"HTTP/1.0 200 "))
    })();
    if !matches!(result, Ok(true)) {
        std::process::exit(1)
    }
}
