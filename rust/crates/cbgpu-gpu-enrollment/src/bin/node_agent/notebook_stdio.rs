//! Hidden sandbox helper: one fixed local Jupyter TCP destination, no command or target arguments.
pub fn run() -> std::io::Result<()> {
    use std::{
        io,
        net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream},
        time::Duration,
    };
    let mut socket = TcpStream::connect_timeout(
        &SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 8888),
        Duration::from_secs(5),
    )?;
    socket.set_nodelay(true)?;
    let mut input = socket.try_clone()?;
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut input);
        let _ = input.shutdown(Shutdown::Write);
    });
    use io::{Read, Write};
    let result = (|| {
        let mut output = io::stdout().lock();
        let mut buffer = [0u8; 8192];
        loop {
            let n = socket.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
            output.flush()?;
        }
        Ok(())
    })();
    let _ = socket.shutdown(Shutdown::Both);
    result
}
