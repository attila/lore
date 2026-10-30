//! The client builds its HTTP agent from the proxy variables in the
//! environment, so these tests change the environment and run in their own
//! process.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use lore::embeddings::Embedder;
use lore::openai_compatible::OpenAiCompatibleClient;

const KEY_VARIABLE: &str = "LORE_TEST_PROXY_KEY";
const EMBEDDING: &str = r#"{"data":[{"embedding":[1.0,0.0],"index":0}]}"#;

/// Read one request head and body, returning the request line and the
/// `Authorization` header.
fn read_request(reader: &mut BufReader<TcpStream>) -> (String, Option<String>) {
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut content_length = 0;
    let mut authorization = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap();
            } else if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.trim().to_string());
            }
        }
    }
    reader.read_exact(&mut vec![0; content_length]).unwrap();
    (request_line.trim().to_string(), authorization)
}

fn answer_embedding(stream: &mut TcpStream) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{EMBEDDING}",
        EMBEDDING.len()
    )
    .unwrap();
}

/// Per connection, the CONNECT line and the tunnelled `Authorization` header.
type Tunnels = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A proxy that accepts every CONNECT and answers the tunnelled request with
/// an embedding itself. Returns its URL and what it saw.
fn serve_proxy() -> (String, Tunnels) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (connect, _) = read_request(&mut reader);
            write!(stream, "HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
            let (_, authorization) = read_request(&mut reader);
            log.lock().unwrap().push((connect, authorization));
            answer_embedding(&mut stream);
        }
    });
    (url, seen)
}

fn serve_embeddings() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            read_request(&mut BufReader::new(stream.try_clone().unwrap()));
            answer_embedding(&mut stream);
        }
    });
    url
}

fn with_proxy_environment(http_proxy: &str, no_proxy: Option<&str>, f: impl FnOnce()) {
    temp_env::with_vars(
        [
            ("ALL_PROXY", None),
            ("all_proxy", None),
            ("HTTPS_PROXY", None),
            ("https_proxy", None),
            ("HTTP_PROXY", Some(http_proxy)),
            ("http_proxy", None),
            ("NO_PROXY", no_proxy),
            ("no_proxy", None),
            (KEY_VARIABLE, Some("sk-through-proxy")),
        ],
        f,
    );
}

#[test]
fn http_proxy_carries_requests_to_a_loopback_server_and_reads_the_key() {
    let (proxy, seen) = serve_proxy();
    with_proxy_environment(&proxy, None, || {
        let client = OpenAiCompatibleClient::new("http://127.0.0.1:9/v1", "m", 8)
            .with_api_key_variable(Some(KEY_VARIABLE));
        assert_eq!(client.embed("hello").unwrap(), [1.0, 0.0]);
    });
    assert_eq!(
        *seen.lock().unwrap(),
        [(
            "CONNECT 127.0.0.1:9 HTTP/1.1".to_string(),
            Some("Bearer sk-through-proxy".to_string())
        )]
    );
}

#[test]
fn no_proxy_sends_requests_straight_to_the_server() {
    let (proxy, seen) = serve_proxy();
    let server = serve_embeddings();
    with_proxy_environment(&proxy, Some("127.0.0.1"), || {
        let client = OpenAiCompatibleClient::new(&format!("{server}/v1"), "m", 8)
            .with_api_key_variable(Some(KEY_VARIABLE));
        assert_eq!(client.embed("hello").unwrap(), [1.0, 0.0]);
    });
    assert!(seen.lock().unwrap().is_empty());
}
