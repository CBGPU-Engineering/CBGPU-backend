//! Docker exec hijack for a fixed read-only Rust proxy. Docker stdout is demultiplexed, never executed.
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub type Pipe = Box<dyn Io>;
pub type Error = &'static str;
pub async fn open(http: &reqwest::Client, container: &str) -> Result<Pipe, Error> {
    if container.len() != 64 || !container.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("CONTAINER_OWNERSHIP_MISMATCH");
    }
    let response=http.post(format!("http://localhost/v1.43/containers/{container}/exec")).json(&json!({"AttachStdin":true,"AttachStdout":true,"AttachStderr":true,"Tty":false,"Privileged":false,"User":"1000:100","Cmd":["/usr/local/bin/cbgpu-node-proxy","__notebook-stdio"]})).send().await.map_err(|_|"DOCKER_UNAVAILABLE")?;
    if !response.status().is_success() {
        return Err("DOCKER_FAILED");
    }
    let bytes = response.bytes().await.map_err(|_| "DOCKER_FAILED")?;
    if bytes.len() > 16384 {
        return Err("DOCKER_FAILED");
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "DOCKER_FAILED")?;
    let id = value["Id"]
        .as_str()
        .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("DOCKER_FAILED")?;
    let response = http
        .post(format!("http://localhost/v1.43/exec/{id}/start"))
        .header("Connection", "Upgrade")
        .header("Upgrade", "tcp")
        .json(&json!({"Detach":false,"Tty":false}))
        .send()
        .await
        .map_err(|_| "DOCKER_UNAVAILABLE")?;
    if response.status() != 101 {
        return Err("DOCKER_HIJACK_UNAVAILABLE");
    }
    let upgraded = response
        .upgrade()
        .await
        .map_err(|_| "DOCKER_HIJACK_UNAVAILABLE")?;
    Ok(adapt(upgraded))
}
pub fn adapt<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(socket: S) -> Pipe {
    let (client, proxy) = tokio::io::duplex(65536);
    tokio::spawn(async move {
        let (mut remote_read, mut remote_write) = tokio::io::split(socket);
        let (mut local_read, mut local_write) = tokio::io::split(proxy);
        let writer = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut local_read, &mut remote_write).await;
            let _ = remote_write.shutdown().await;
        });
        let _ = async {
            let mut stderr = 0usize;
            loop {
                let mut header = [0; 8];
                match remote_read.read_exact(&mut header).await {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(_) => return Err(()),
                };
                if header[1..4] != [0, 0, 0] || ![1, 2].contains(&header[0]) {
                    return Err(());
                }
                let mut remaining =
                    u32::from_be_bytes(header[4..8].try_into().map_err(|_| ())?) as usize;
                if remaining > 8 * 1024 * 1024 {
                    return Err(());
                }
                if header[0] == 2 {
                    stderr = stderr.saturating_add(remaining);
                    if stderr > 65536 {
                        return Err(());
                    }
                }
                let mut chunk = [0; 8192];
                while remaining > 0 {
                    let len = remaining.min(chunk.len());
                    remote_read
                        .read_exact(&mut chunk[..len])
                        .await
                        .map_err(|_| ())?;
                    if header[0] == 1 {
                        local_write.write_all(&chunk[..len]).await.map_err(|_| ())?;
                    }
                    remaining -= len;
                }
            }
            Ok::<(), ()>(())
        }
        .await;
        let _ = local_write.shutdown().await;
        writer.abort();
    });
    Box::new(client)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn demux_preserves_raw_binary_and_stdin_without_stderr_leaks() {
        let (client, mut daemon) = tokio::io::duplex(65536);
        let mut pipe = adapt(client);
        let task = tokio::spawn(async move {
            let mut request = [0; 3];
            daemon.read_exact(&mut request).await.unwrap();
            assert_eq!(request, [0, 255, 17]);
            for (kind, body) in [
                (2, b"private stderr".as_slice()),
                (1, [0, 128, 255].as_slice()),
            ] {
                let mut h = [0; 8];
                h[0] = kind;
                h[4..].copy_from_slice(&(body.len() as u32).to_be_bytes());
                daemon.write_all(&h).await.unwrap();
                daemon.write_all(body).await.unwrap();
            }
            daemon.shutdown().await.unwrap();
        });
        pipe.write_all(&[0, 255, 17]).await.unwrap();
        let mut got = vec![];
        pipe.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, [0, 128, 255]);
        task.await.unwrap();
    }
}
