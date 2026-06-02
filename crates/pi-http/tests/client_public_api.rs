use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use futures::StreamExt;
use pi_http::http::client::Client;
use serde_json::json;

struct TestServer {
    url: String,
    request_rx: mpsc::Receiver<CapturedRequest>,
    done_rx: mpsc::Receiver<()>,
}

#[derive(Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl CapturedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .rev()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.as_str()))
    }
}

fn run_async<T>(future: impl Future<Output = T>) -> T {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("pi-http-test")
                .build()
                .expect("build Tokio test runtime")
        })
        .block_on(future)
}

#[test]
fn client_uses_caller_tokio_runtime_without_owning_one() {
    run_async(async {
        let client = Client::new();
        drop(client);
    });
}

fn spawn_server(handler: impl FnOnce(CapturedRequest, TcpStream) + Send + 'static) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let url = format!("http://{}", listener.local_addr().expect("local addr"));
    let (request_tx, request_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();

    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept request");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let request = read_request(&mut reader).expect("read request");
        request_tx
            .send(clone_request(&request))
            .expect("send request");
        handler(request, stream);
        let _ = done_tx.send(());
    });

    TestServer {
        url,
        request_rx,
        done_rx,
    }
}

fn clone_request(request: &CapturedRequest) -> CapturedRequest {
    CapturedRequest {
        method: request.method.clone(),
        path: request.path.clone(),
        headers: request.headers.clone(),
        body: request.body.clone(),
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<CapturedRequest> {
    let mut first_line = String::new();
    reader.read_line(&mut first_line)?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let content_length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or_default();
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;

    Ok(CapturedRequest {
        method,
        path,
        headers,
        body,
    })
}

fn write_response(mut stream: TcpStream, response: &[u8]) {
    stream.write_all(response).expect("write response");
    stream.flush().expect("flush response");
}

#[test]
fn get_text_response_preserves_status_headers_and_body() {
    let server = spawn_server(|_, stream| {
        write_response(
            stream,
            b"HTTP/1.1 201 Created\r\nContent-Type: text/plain\r\nX-Test: yes\r\nContent-Length: 11\r\n\r\nhello world",
        );
    });

    let response = run_async(Client::new().get(&format!("{}/hello", server.url)).send())
        .expect("send GET request");
    assert_eq!(response.status(), 201);
    assert_eq!(
        response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-test"))
            .map(|(_, value)| value.as_str()),
        Some("yes")
    );
    let text = run_async(response.text()).expect("read response text");
    assert_eq!(text, "hello world");

    let request = server.request_rx.recv().expect("captured request");
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/hello");
    assert!(request.body.is_empty());
    server
        .done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("server completed");
}

#[test]
fn post_json_sends_body_and_replaces_duplicate_headers_case_insensitively() {
    let server = spawn_server(|_, stream| {
        write_response(stream, b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
    });

    let response = run_async(
        Client::new()
            .post(&format!("{}/v1/messages", server.url))
            .header("Authorization", "Bearer old")
            .header("authorization", "Bearer new")
            .json(&json!({"model": "test-model", "input": "hello"}))
            .expect("build json request")
            .send(),
    )
    .expect("send POST request");
    assert_eq!(run_async(response.text()).expect("read body"), "ok");

    let request = server.request_rx.recv().expect("captured request");
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/messages");
    assert_eq!(request.header("authorization"), Some("Bearer new"));
    assert_eq!(request.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("json body");
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["input"], "hello");
    assert_eq!(
        request
            .header("content-length")
            .and_then(|value| value.parse::<usize>().ok()),
        Some(request.body.len())
    );
}

#[test]
fn chunked_response_is_exposed_as_byte_stream() {
    let server = spawn_server(|_, stream| {
        write_response(
            stream,
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n1\r\n \r\n5\r\nworld\r\n0\r\n\r\n",
        );
    });

    let response = run_async(Client::new().get(&format!("{}/stream", server.url)).send())
        .expect("send streaming request");
    let chunks = run_async(async {
        response
            .bytes_stream()
            .map(|chunk| chunk.expect("stream chunk"))
            .collect::<Vec<_>>()
            .await
    });
    let body = chunks.concat();
    assert_eq!(String::from_utf8(body).expect("utf8 body"), "hello world");

    let request = server.request_rx.recv().expect("captured request");
    assert_eq!(request.path, "/stream");
}

#[test]
fn request_timeout_fails_when_server_delays_headers() {
    let server = spawn_server(|_, stream| {
        thread::sleep(Duration::from_millis(200));
        write_response(stream, b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nslow");
    });

    let result = run_async(
        Client::new()
            .get(&format!("{}/slow", server.url))
            .timeout(Duration::from_millis(25))
            .send(),
    );
    let error = match result {
        Ok(_) => panic!("request should time out"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Request timed out"));

    let request = server.request_rx.recv().expect("captured request");
    assert_eq!(request.path, "/slow");
}
