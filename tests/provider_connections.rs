//! A warm native provider must reuse the same TCP connection for successive
//! HTTP/1.1 requests, including changes between streaming and JSON responses.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aishe::providers::{
    anthropic::AnthropicProvider, openai_compat::OpenAiProvider, Msg, Provider, ResponseFormat,
};
use serde_json::Value;
use tiny_http::{Header, Response, Server, StatusCode};

const CHAT_PATH: &str = "/v1/chat/completions";
const RESPONSES_PATH: &str = "/v1/responses";
const ANTHROPIC_PATH: &str = "/v1/messages";
const CHAT_JSON: &str = r#"{"choices":[{"message":{"content":"hello"}}]}"#;
const CHAT_SSE: &str =
    "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n";
const RESPONSES_JSON: &str =
    r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"hello"}]}]}"#;
const RESPONSES_SSE: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}]}}\n\n",
);
const ANTHROPIC_JSON: &str = r#"{"content":[{"type":"text","text":"hello"}]}"#;
const ANTHROPIC_SSE: &str = "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n";

struct Reply {
    path: &'static str,
    body: &'static str,
    stream: bool,
    status: u16,
}

impl Reply {
    fn json(path: &'static str, body: &'static str) -> Self {
        Self {
            path,
            body,
            stream: false,
            status: 200,
        }
    }

    fn sse(path: &'static str, body: &'static str) -> Self {
        Self {
            path,
            body,
            stream: true,
            status: 200,
        }
    }
}

/// Record the peer address on every request accepted by the loopback server.
/// Distinct source ports identify distinct TCP connections to this listener.
fn keepalive_server(replies: Vec<Reply>) -> (String, JoinHandle<Vec<SocketAddr>>) {
    let server = Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr());
    let worker = thread::spawn(move || {
        let mut connections = Vec::new();
        for reply in replies {
            let mut request = server
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .expect("provider did not send the expected request");
            connections.push(*request.remote_addr().unwrap());
            assert_eq!(request.url(), reply.path);
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let body: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(body["stream"].as_bool().unwrap_or(false), reply.stream);
            let content_type = if reply.stream && reply.status == 200 {
                "text/event-stream"
            } else {
                "application/json"
            };
            let response = Response::from_string(reply.body)
                .with_status_code(StatusCode(reply.status))
                .with_header(Header::from_bytes("Content-Type", content_type).unwrap());
            request.respond(response).unwrap();
        }
        connections
    });
    (url, worker)
}

fn exercise_completion_modes(provider: &dyn Provider) {
    let messages = [Msg::User("hi".into())];
    assert_eq!(
        provider
            .complete("SYS", &messages, &ResponseFormat::Text)
            .unwrap(),
        "hello"
    );
    let completion = provider.complete_with_tools("SYS", &messages, &[]).unwrap();
    assert_eq!(completion.text.as_deref(), Some("hello"));
    let mut text = String::new();
    assert_eq!(
        provider
            .complete_stream("SYS", &messages, &ResponseFormat::Text, &mut |delta| {
                text.push_str(delta)
            })
            .unwrap(),
        "hello"
    );
    assert_eq!(text, "hello");
    text.clear();
    let completion = provider
        .complete_with_tools_stream("SYS", &messages, &[], &mut |delta| text.push_str(delta))
        .unwrap();
    assert_eq!(completion.text.as_deref(), Some("hello"));
    assert_eq!(text, "hello");
}

fn assert_one_connection(server: JoinHandle<Vec<SocketAddr>>) {
    let connections = server.join().unwrap();
    assert!(connections.iter().all(|peer| peer == &connections[0]));
}

#[test]
fn openai_chat_reuses_connection_for_completions_streams_and_embeddings() {
    let (url, server) = keepalive_server(vec![
        Reply::json(CHAT_PATH, CHAT_JSON),
        Reply::json(CHAT_PATH, CHAT_JSON),
        Reply::sse(CHAT_PATH, CHAT_SSE),
        Reply::sse(CHAT_PATH, CHAT_SSE),
        Reply::json("/v1/embeddings", r#"{"data":[{"embedding":[0.25,0.5]}]}"#),
    ]);
    let provider = OpenAiProvider::new(url, "key".into(), "model".into());
    exercise_completion_modes(&provider);
    assert_eq!(
        provider.embed(&["hi".into()], "embedding-model").unwrap(),
        vec![vec![0.25, 0.5]]
    );
    assert_one_connection(server);
}

#[test]
fn openai_responses_reuses_connection_for_completions_and_streams() {
    let (url, server) = keepalive_server(vec![
        Reply::json(RESPONSES_PATH, RESPONSES_JSON),
        Reply::json(RESPONSES_PATH, RESPONSES_JSON),
        Reply::sse(RESPONSES_PATH, RESPONSES_SSE),
        Reply::sse(RESPONSES_PATH, RESPONSES_SSE),
    ]);
    let provider =
        OpenAiProvider::with_options(url, "key".into(), "model".into(), "responses", "auto");
    exercise_completion_modes(&provider);
    assert_one_connection(server);
}

#[test]
fn anthropic_reuses_connection_for_completions_and_streams() {
    let (url, server) = keepalive_server(vec![
        Reply::json(ANTHROPIC_PATH, ANTHROPIC_JSON),
        Reply::json(ANTHROPIC_PATH, ANTHROPIC_JSON),
        Reply::sse(ANTHROPIC_PATH, ANTHROPIC_SSE),
        Reply::sse(ANTHROPIC_PATH, ANTHROPIC_SSE),
    ]);
    let provider = AnthropicProvider::new(url, "key".into(), "model".into());
    exercise_completion_modes(&provider);
    assert_one_connection(server);
}

#[test]
fn successful_retries_reuse_the_connection_for_subsequent_calls() {
    for anthropic in [false, true] {
        for stream in [false, true] {
            let (path, json, sse) = if anthropic {
                (ANTHROPIC_PATH, ANTHROPIC_JSON, ANTHROPIC_SSE)
            } else {
                (CHAT_PATH, CHAT_JSON, CHAT_SSE)
            };
            let (url, server) = keepalive_server(vec![
                Reply {
                    path,
                    body: r#"{"error":{"message":"try again"}}"#,
                    stream,
                    status: 503,
                },
                Reply {
                    path,
                    body: if stream { sse } else { json },
                    stream,
                    status: 200,
                },
                Reply::json(path, json),
            ]);
            let provider: Box<dyn Provider> = if anthropic {
                Box::new(AnthropicProvider::new(url, "key".into(), "model".into()))
            } else {
                Box::new(OpenAiProvider::new(url, "key".into(), "model".into()))
            };
            let messages = [Msg::User("hi".into())];
            let result = if stream {
                provider.complete_stream("SYS", &messages, &ResponseFormat::Text, &mut |_| {})
            } else {
                provider.complete("SYS", &messages, &ResponseFormat::Text)
            };
            assert_eq!(result.unwrap(), "hello");
            assert_eq!(
                provider
                    .complete("SYS", &messages, &ResponseFormat::Text)
                    .unwrap(),
                "hello"
            );
            let connections = server.join().unwrap();
            assert_eq!(connections.len(), 3);
            assert_eq!(connections[1], connections[2]);
        }
    }
}

fn accept_with_timeout(listener: &TcpListener, timeout: Duration) -> TcpStream {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Darwin inherits the listener's O_NONBLOCK on accepted
                // sockets; Linux does not. Only accept polling is nonblocking.
                // Request reads retain their existing five-second deadline.
                stream.set_nonblocking(false).unwrap();
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "provider did not retry promptly");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    }
}

fn read_http_request(stream: TcpStream) -> TcpStream {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut length = 0;
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
        }
    }
    reader.read_exact(&mut vec![0; length]).unwrap();
    reader.into_inner()
}

#[test]
fn retries_do_not_wait_for_stalled_error_bodies() {
    for anthropic in [false, true] {
        for stream in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let mut failed =
                    read_http_request(accept_with_timeout(&listener, Duration::from_secs(5)));
                // Keep this connection open with an incomplete body until the
                // next attempt arrives. Draining it would delay the retry.
                failed
                    .write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 1024\r\n\r\n{")
                    .unwrap();
                let retry_started = Instant::now();
                let mut success =
                    read_http_request(accept_with_timeout(&listener, Duration::from_secs(2)));
                let waited = retry_started.elapsed();
                let body = match (anthropic, stream) {
                    (false, false) => CHAT_JSON,
                    (false, true) => CHAT_SSE,
                    (true, false) => ANTHROPIC_JSON,
                    (true, true) => ANTHROPIC_SSE,
                };
                write!(
                    success,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                waited
            });
            let provider: Box<dyn Provider> = if anthropic {
                Box::new(AnthropicProvider::new(url, "key".into(), "model".into()))
            } else {
                Box::new(OpenAiProvider::new(url, "key".into(), "model".into()))
            };
            let messages = [Msg::User("hi".into())];
            let result = if stream {
                provider.complete_stream("SYS", &messages, &ResponseFormat::Text, &mut |_| {})
            } else {
                provider.complete("SYS", &messages, &ResponseFormat::Text)
            };
            assert_eq!(result.unwrap(), "hello");
            assert!(server.join().unwrap() < Duration::from_secs(2));
        }
    }
}
