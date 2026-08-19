//! A tiny recording HTTP server, so the end-to-end tests can assert on exactly what
//! went out on the wire without needing a network or an external process.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Captured {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Captured {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| {
            panic!("body is not JSON ({e}): {}", self.body);
        })
    }

    /// Form-encoded bodies, including Slack-style `payload=<json>`.
    pub fn form(&self) -> HashMap<String, String> {
        self.body
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (urldecode(k), urldecode(v))
            })
            .collect()
    }
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub struct MockServer {
    pub base_url: String,
    captured: Arc<Mutex<Vec<Captured>>>,
    rx: Receiver<()>,
}

impl MockServer {
    /// Answers every request with `status`.
    pub fn start(status: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let port = listener.local_addr().unwrap().port();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = captured.clone();
        let (tx, rx) = channel();

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if let Some(c) = handle(stream, status) {
                    sink.lock().unwrap().push(c);
                    let _ = tx.send(());
                }
            }
        });

        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            captured,
            rx,
        }
    }

    /// Wait until `n` requests have arrived, or give up after a few seconds.
    pub fn wait_for(&self, n: usize) {
        while self.captured.lock().unwrap().len() < n {
            if self.rx.recv_timeout(Duration::from_secs(10)).is_err() {
                break;
            }
        }
    }

    pub fn requests(&self) -> Vec<Captured> {
        self.captured.lock().unwrap().clone()
    }

    /// The single request whose path contains `needle`.
    pub fn request_for(&self, needle: &str) -> Captured {
        let all = self.requests();
        let matches: Vec<&Captured> = all.iter().filter(|c| c.path.contains(needle)).collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one request matching '{needle}', got {}: {:?}",
            matches.len(),
            all.iter().map(|c| &c.path).collect::<Vec<_>>()
        );
        matches[0].clone()
    }

    pub fn requests_for(&self, needle: &str) -> Vec<Captured> {
        self.requests()
            .into_iter()
            .filter(|c| c.path.contains(needle))
            .collect()
    }
}

fn handle(mut stream: TcpStream, status: u16) -> Option<Captured> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body).ok()?;
    }

    let response =
        format!("HTTP/1.1 {status} OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}");
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();

    Some(Captured {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}
