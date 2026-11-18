//! Bounded HTTP over an authenticated Docker-exec byte pipe; never TCP/DNS from caller input.
use super::notebook_docker_stream::Pipe;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, body::Incoming};
use hyper_util::rt::TokioIo;
pub async fn request(
    pipe: Pipe,
    request: Request<Full<Bytes>>,
) -> Result<Response<Incoming>, &'static str> {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(pipe))
        .await
        .map_err(|_| "UPSTREAM_UNAVAILABLE")?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    sender
        .send_request(request)
        .await
        .map_err(|_| "UPSTREAM_UNAVAILABLE")
}
pub async fn body(mut body: Incoming, limit: usize) -> Result<Vec<u8>, &'static str> {
    let mut bytes = vec![];
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| "UPSTREAM_UNAVAILABLE")?;
        if let Ok(data) = frame.into_data() {
            if bytes.len() + data.len() > limit {
                return Err("RELAY_BODY_TOO_LARGE");
            }
            bytes.extend(data)
        }
    }
    Ok(bytes)
}
