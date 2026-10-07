//! Helpers shared by tests.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

/// Every request a [`serve`]d server received: its header block and its body.
pub(crate) type Requests = Arc<Mutex<Vec<(String, String)>>>;

/// A tiny local HTTP server that answers each request with the next canned
/// `(status, content type, body)`. Returns its base URL (`http://127.0.0.1:port`) and the
/// requests it received (recorded before each reply is sent).
pub(crate) fn serve_full(responses: Vec<(u16, &'static str, &'static str)>) -> (String, Requests) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let seen: Requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    std::thread::spawn(move || {
        for (status, content_type, body) in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut length = 0usize;
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
            }
            let mut payload = vec![0u8; length];
            reader.read_exact(&mut payload).expect("body");
            recorded
                .lock()
                .unwrap()
                .push((head, String::from_utf8_lossy(&payload).into_owned()));
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (format!("http://{address}"), seen)
}

/// Like [`serve_full`], for tests that only look at request bodies via `seen.lock().unwrap()[i]`.
pub(crate) fn serve(responses: Vec<(u16, &'static str, &'static str)>) -> (String, BodyList) {
    let (url, seen) = serve_full(responses);
    (url, BodyList(seen))
}

/// A view of the recorded requests that indexes straight to the body text.
pub(crate) struct BodyList(Requests);

impl BodyList {
    pub(crate) fn lock(&self) -> BodyView {
        BodyView(
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|(_, body)| body.clone())
                .collect(),
        )
    }
}

/// A snapshot of the bodies received so far.
pub(crate) struct BodyView(Vec<String>);

impl BodyView {
    pub(crate) fn unwrap(self) -> Vec<String> {
        self.0
    }
}
