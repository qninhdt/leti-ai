//! TCP/HTTP/1.1 hand-rolled server. No axum, no hyper.
//!
//! Accepts one connection at a time on a per-connection task. Parses the
//! request line + headers, reads `content-length` body bytes, hands the
//! parsed request to a scenario dispatcher, then writes the response
//! bytes (status line + headers + body) and closes the connection.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::scenarios::{Response, Scenario, detect_scenario};

/// One captured inbound request. Tests assert on this to verify the
/// adapter built the right wire shape.
#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub scenario: Option<Scenario>,
    pub body: String,
}

/// Mock server handle. Drop kills the accept task.
pub struct MockOpenAiService {
    base_url: String,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    shutdown: Option<oneshot::Sender<()>>,
    accept_task: Option<JoinHandle<()>>,
}

impl MockOpenAiService {
    /// Bind to `127.0.0.1:0` and start accepting. Returns once the
    /// listener is ready (so `base_url()` is immediately usable).
    pub async fn spawn() -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let base_url = format!("http://{addr}/v1");
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let (tx, rx) = oneshot::channel();

        let cap_for_task = Arc::clone(&captured);
        let task = tokio::spawn(async move {
            run_accept_loop(listener, cap_for_task, rx).await;
        });

        Ok(Self {
            base_url,
            captured,
            shutdown: Some(tx),
            accept_task: Some(task),
        })
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    #[must_use]
    pub fn captured_requests(&self) -> Vec<CapturedRequest> {
        self.captured.lock().expect("captured mutex").clone()
    }
}

impl Drop for MockOpenAiService {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.accept_task.take() {
            task.abort();
        }
    }
}

async fn run_accept_loop(
    listener: TcpListener,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    mut shutdown: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown => return,
            accepted = listener.accept() => {
                let Ok((stream, _peer)) = accepted else {
                    continue;
                };
                let cap = Arc::clone(&captured);
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, cap).await {
                        eprintln!("mock-openai-service: connection error: {e}");
                    }
                });
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
) -> io::Result<()> {
    let req = read_request(&mut stream).await?;

    let body_json: serde_json::Value =
        serde_json::from_str(&req.body).unwrap_or(serde_json::Value::Null);
    let scenario = body_json
        .get("messages")
        .and_then(detect_scenario)
        .unwrap_or(Scenario::SimpleText);

    captured
        .lock()
        .expect("captured mutex")
        .push(CapturedRequest {
            method: req.method.clone(),
            path: req.path.clone(),
            headers: req.headers.clone(),
            scenario: Some(scenario),
            body: req.body.clone(),
        });

    let response = scenario.render();
    write_response(&mut stream, response).await?;
    let _ = stream.shutdown().await;
    Ok(())
}

#[derive(Debug)]
struct ParsedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

async fn read_request(stream: &mut TcpStream) -> io::Result<ParsedRequest> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    let header_end;
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "headers"));
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            header_end = pos;
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "headers too large",
            ));
        }
    }

    let head = std::str::from_utf8(&buf[..header_end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 headers"))?;
    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut headers = Vec::new();
    let mut content_length: usize = 0;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_string();
            if k == "content-length" {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }

    let body_start = header_end + 4;
    let mut body = buf[body_start..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    let body = String::from_utf8(body)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 body"))?;

    Ok(ParsedRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_response(stream: &mut TcpStream, resp: Response) -> io::Result<()> {
    match resp {
        Response::Sse {
            chunks,
            inter_chunk_delay,
        } => {
            let head = b"HTTP/1.1 200 OK\r\n\
                content-type: text/event-stream\r\n\
                cache-control: no-cache\r\n\
                connection: close\r\n\
                transfer-encoding: chunked\r\n\r\n";
            stream.write_all(head).await?;
            for chunk in chunks {
                let size_line = format!("{:x}\r\n", chunk.len());
                stream.write_all(size_line.as_bytes()).await?;
                stream.write_all(&chunk).await?;
                stream.write_all(b"\r\n").await?;
                stream.flush().await?;
                if inter_chunk_delay > Duration::ZERO {
                    tokio::time::sleep(inter_chunk_delay).await;
                }
            }
            stream.write_all(b"0\r\n\r\n").await?;
        }
        Response::Error {
            status,
            status_text,
            body,
            extra_headers,
        } => {
            let mut head = format!(
                "HTTP/1.1 {status} {status_text}\r\n\
                content-type: application/json\r\n\
                connection: close\r\n\
                content-length: {}\r\n",
                body.len()
            );
            for (k, v) in extra_headers {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            head.push_str("\r\n");
            stream.write_all(head.as_bytes()).await?;
            stream.write_all(body.as_bytes()).await?;
        }
    }
    stream.flush().await?;
    Ok(())
}
