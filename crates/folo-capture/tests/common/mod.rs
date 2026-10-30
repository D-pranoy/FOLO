//! A fake node for the agent's tests (ADR-0037 D22): it accepts the agent's
//! WebSocket connections on loopback and speaks the node's end of the
//! capture link.

use std::time::Duration;

use folo_capture::environment::Environment;
use folo_capture_core::codec;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::{Message, http::HeaderValue};
use tokio_tungstenite::{WebSocketStream, accept_hdr_async};

pub const TOKEN: &str = "launch-token";

/// A node that accepts the agent's connections.
pub struct FakeNode {
    pub listener: TcpListener,
    pub url: String,
}

#[allow(dead_code)] // each test file uses some of it
impl FakeNode {
    pub async fn bind() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "ws://{}/api/v1/capture-link",
            listener.local_addr().unwrap()
        );
        Self { listener, url }
    }

    pub fn environment(&self) -> Environment {
        Environment {
            link: self.url.clone(),
            token: TOKEN.into(),
        }
    }

    pub async fn accept(&self) -> Link {
        let (stream, _) = self.listener.accept().await.unwrap();
        let mut protocol = None;
        // The error type is the library's: a large one the closure can't change
        #[allow(clippy::result_large_err)]
        let callback = |request: &Request, mut response: Response| {
            protocol = request
                .headers()
                .get("Sec-WebSocket-Protocol")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            response.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                HeaderValue::from_static("folo.capture.v1"),
            );
            Ok(response)
        };
        let socket = accept_hdr_async(stream, callback).await.unwrap();
        Link {
            socket,
            protocol,
            next_id: 1,
            statuses: Vec::new(),
            frames: Vec::new(),
        }
    }
}

/// The node's end of one connection.
pub struct Link {
    pub socket: WebSocketStream<TcpStream>,
    #[allow(dead_code)] // the binary's test doesn't read it
    pub protocol: Option<String>,
    next_id: u64,
    /// `capture.status` params and frame messages seen while waiting for something else.
    statuses: Vec<Value>,
    pub frames: Vec<Vec<u8>>,
}

#[allow(dead_code)]
impl Link {
    pub async fn hello(&mut self) -> Value {
        match self.socket.next().await {
            Some(Ok(Message::Text(text))) => serde_json::from_str(text.as_str()).unwrap(),
            other => panic!("no hello: {other:?}"),
        }
    }

    pub async fn welcome(&mut self, result: &str) {
        let body = json!({"version": "1", "result": result});
        self.socket
            .send(Message::text(body.to_string()))
            .await
            .unwrap();
    }

    /// The hello checked and welcomed.
    pub async fn welcomed(&mut self) {
        let hello = self.hello().await;
        assert_eq!(hello["token"], TOKEN);
        self.welcome("ok").await;
    }

    pub async fn send_request(&mut self, method: &str, params: &Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut bytes = vec![0x01];
        bytes.extend_from_slice(body.to_string().as_bytes());
        self.socket.send(Message::binary(bytes)).await.unwrap();
        id
    }

    /// Reads one message; `Ok` for a JSON-RPC one, `Err` with a frame's bytes.
    pub async fn read(&mut self) -> Option<Result<Value, Vec<u8>>> {
        loop {
            match self.socket.next().await? {
                Ok(Message::Binary(bytes)) if bytes[0] == 0x01 => {
                    return Some(Ok(serde_json::from_slice(&bytes[1..]).unwrap()));
                }
                Ok(Message::Binary(bytes)) if bytes[0] == 0x04 => return Some(Err(bytes.to_vec())),
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }

    /// Reads until something else than a status or a frame arrives,
    /// keeping those aside.
    pub async fn next_other(&mut self) -> Option<Value> {
        loop {
            match self.read().await? {
                Ok(message) if message["method"] == "capture.status" => {
                    self.statuses.push(message["params"].clone());
                }
                Ok(message) => return Some(message),
                Err(frame) => self.frames.push(frame),
            }
        }
    }

    /// A request and its answer: `result` or `error`.
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, &params).await;
        let answer = tokio::time::timeout(Duration::from_secs(5), self.next_other())
            .await
            .expect("an answer in 5 s")
            .expect("the link stays open");
        assert_eq!(answer["id"], id);
        answer
    }

    /// The next `capture.status` params.
    pub async fn status(&mut self) -> Value {
        if !self.statuses.is_empty() {
            return self.statuses.remove(0);
        }
        loop {
            match self.read().await.expect("the link stays open") {
                Ok(message) if message["method"] == "capture.status" => {
                    return message["params"].clone();
                }
                Ok(other) => panic!("unexpected message {other}"),
                Err(frame) => self.frames.push(frame),
            }
        }
    }

    /// Statuses until one satisfies `wanted`.
    pub async fn status_where(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = self.status().await;
                if wanted(&status) {
                    return status;
                }
            }
        })
        .await
        .expect("the status in 5 s")
    }

    /// The next frame message, decoded.
    pub async fn frame(&mut self) -> folo_capture_core::types::ScreenFrame {
        let bytes = if self.frames.is_empty() {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Err(frame) = self.read().await.expect("the link stays open") {
                        return frame;
                    }
                }
            })
            .await
            .expect("a frame in 5 s")
        } else {
            self.frames.remove(0)
        };
        assert_eq!(bytes[0], 0x04);
        codec::decode(&bytes[1..]).expect("a frame the codec reads")
    }
}
