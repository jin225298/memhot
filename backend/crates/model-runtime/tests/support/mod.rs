//! 测试支持：手写 mock HTTP server 与静态配置源。
//!
//! workspace 不引入 wiremock；用 tokio TcpListener 实现按序弹出的
//! 预设响应序列，支持断连模拟与请求记录断言。每连接处理一个请求
//! （connection: close），reqwest 默认每请求新建连接，行为一致。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use model_runtime::catalog::{CatalogError, ModelConfigSource, ModelTarget};

/// 预设响应。
#[derive(Debug, Clone)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// 响应头写一半后直接断开（模拟断流）。
    pub drop_connection: bool,
    /// 记录请求后不响应，直到客户端断开（模拟挂起）。
    pub hang: bool,
}

impl MockResponse {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: body.into(),
            drop_connection: false,
            hang: false,
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn drop_connection() -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: String::new(),
            drop_connection: true,
            hang: false,
        }
    }

    /// 接受请求后永不响应（确定性超时测试）。
    pub fn hang() -> Self {
        Self {
            status: 0,
            headers: vec![],
            body: String::new(),
            drop_connection: false,
            hang: true,
        }
    }
}

/// 记录到的一次请求。
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    #[allow(dead_code)] // 诊断保留
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Default)]
struct Shared {
    queue: VecDeque<MockResponse>,
    requests: Vec<RecordedRequest>,
    /// 队列耗尽后继续断连（应对 hyper 对断连的透明重试，保证确定性）。
    drop_when_empty: bool,
}

/// 单连接顺序响应的 mock server。每个请求按队列弹出下一个响应。
pub struct MockServer {
    shared: Arc<Mutex<Shared>>,
    pub addr: std::net::SocketAddr,
    shutdown: Arc<AtomicBool>,
}

impl MockServer {
    pub async fn spawn() -> Self {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");
        let shutdown = Arc::new(AtomicBool::new(false));
        let task_shared = Arc::clone(&shared);
        let task_shutdown = Arc::clone(&shutdown);
        tokio::spawn(async move {
            loop {
                let accept = listener.accept();
                let stopped = task_shutdown.load(Ordering::SeqCst);
                if stopped {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                        if task_shutdown.load(Ordering::SeqCst) { break; }
                    }
                    accepted = accept => {
                        let Ok((stream, _)) = accepted else { continue };
                        let shared = Arc::clone(&task_shared);
                        tokio::spawn(async move {
                            if let Err(error) = serve_connection(stream, shared).await {
                                eprintln!("mock server connection error: {error}");
                            }
                        });
                    }
                }
            }
        });
        Self {
            shared,
            addr,
            shutdown,
        }
    }

    /// 追加预设响应（按请求顺序消费）。
    pub fn enqueue(&self, response: MockResponse) {
        self.shared.lock().unwrap().queue.push_back(response);
    }

    /// 已收到的请求快照。
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.shared.lock().unwrap().requests.clone()
    }

    /// 队列耗尽后对后续请求一律断连（确定性网络错误）。
    pub fn drop_when_empty(&self) {
        self.shared.lock().unwrap().drop_when_empty = true;
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}/v1", self.addr)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

async fn serve_connection(
    mut stream: tokio::net::TcpStream,
    shared: Arc<Mutex<Shared>>,
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buffer = [0u8; 64 * 1024];
    let mut raw = Vec::new();
    // 读到 header 结束或 EOF。
    loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
        if raw.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let mut lines = raw.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut method = String::new();
    let mut path = String::new();
    let mut words = request_line.split_whitespace();
    if let Some(first) = words.next() {
        method = first.to_owned();
    }
    if let Some(second) = words.next() {
        path = second.to_owned();
    }
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if name == "content-length" {
                content_length = value.parse().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    let header_end = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
    let mut body = raw[header_end.min(raw.len())..].to_owned();
    while body.len() < content_length {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        body.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }

    shared.lock().unwrap().requests.push(RecordedRequest {
        method,
        path,
        headers,
        body,
    });

    enum Popped {
        Resp(MockResponse),
        DropAll,
    }
    let popped = {
        let mut guard = shared.lock().unwrap();
        match guard.queue.pop_front() {
            Some(response) => Popped::Resp(response),
            None if guard.drop_when_empty => Popped::DropAll,
            None => Popped::Resp(MockResponse::json(500, "{\"error\":\"mock queue empty\"}")),
        }
    };
    if let Popped::DropAll = popped {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n")
            .await?;
        stream.shutdown().await?;
        return Ok(());
    }
    let response = match popped {
        Popped::Resp(response) => response,
        Popped::DropAll => unreachable!(),
    };

    if response.hang {
        // 记录请求后等待客户端断开（读返回 0）。
        loop {
            let read = stream.read(&mut buffer).await?;
            if read == 0 {
                return Ok(());
            }
        }
    }

    if response.drop_connection {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n")
            .await?;
        stream.shutdown().await?;
        return Ok(());
    }

    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        504 => "Gateway Timeout",
        _ => "Status",
    };
    let mut out = format!("HTTP/1.1 {} {}\r\n", response.status, reason);
    for (name, value) in &response.headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n{}",
        response.body.len(),
        response.body
    ));
    stream.write_all(out.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

/// 静态配置源：固定 target 列表 + 凭据表。
pub struct StaticSource {
    pub targets: Vec<ModelTarget>,
    pub credentials: Vec<(String, String)>,
}

impl StaticSource {
    pub fn new(targets: Vec<ModelTarget>, credentials: Vec<(String, String)>) -> Self {
        Self {
            targets,
            credentials,
        }
    }
}

impl ModelConfigSource for StaticSource {
    fn list_targets(&self) -> Result<Vec<ModelTarget>, CatalogError> {
        Ok(self.targets.clone())
    }

    fn credential(&self, target_id: &str) -> Result<Option<String>, CatalogError> {
        Ok(self
            .credentials
            .iter()
            .find(|(id, _)| id == target_id)
            .map(|(_, key)| key.clone()))
    }
}
