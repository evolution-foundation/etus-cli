use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Instant, interval_at, sleep, timeout, timeout_at};
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL, USER_AGENT};
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self, Message};

use super::protocol::{
    self, Control, DATA, END, HEADER_LEN, MAX_DATA, MAX_STREAMS, OPEN, OPENED, REFUSED, RESET, Retry, Targets,
};
use super::state::{Reporter, State};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(25);
const IDLE_LIMIT: Duration = Duration::from_secs(60);
const STALLED_STREAM: Duration = Duration::from_secs(30);
const STALLED_SOCKET: Duration = Duration::from_secs(60);
const HEARTBEAT_PINGS: u32 = 24;
const OUT_QUEUE: usize = 32;
const STREAM_QUEUE: usize = 4;

pub struct Dial {
    pub url: String,
    pub key: String,
    pub device_name: String,
    pub targets: Targets,
}

pub struct Ended {
    pub retry: Retry,
    pub greeted: bool,
}

pub fn tls_connector() -> Connector {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Connector::Rustls(Arc::new(config))
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(64 * 1024)
        .max_write_buffer_size(2 * 1024 * 1024)
        .max_message_size(Some(HEADER_LEN + 2 * MAX_DATA))
        .max_frame_size(Some(HEADER_LEN + 2 * MAX_DATA))
}

fn request(dial: &Dial) -> Result<tungstenite::handshake::client::Request, String> {
    let mut request = dial
        .url
        .as_str()
        .into_client_request()
        .map_err(|_| "endereço do túnel inválido".to_string())?;
    let mut bearer = HeaderValue::from_str(&format!("Bearer {}", dial.key))
        .map_err(|_| "chave com caracteres inválidos".to_string())?;
    bearer.set_sensitive(true);
    let headers = request.headers_mut();
    headers.insert(AUTHORIZATION, bearer);
    headers.insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(protocol::SUBPROTOCOL));
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(concat!("etus-cli/", env!("CARGO_PKG_VERSION"))),
    );
    if let Ok(name) = HeaderValue::from_bytes(dial.device_name.as_bytes()) {
        headers.insert(HeaderName::from_static("x-etus-device-name"), name);
    }
    Ok(request)
}

fn short_error(error: &tungstenite::Error) -> String {
    match error {
        tungstenite::Error::Io(e) => format!("erro de rede ({})", e.kind()),
        tungstenite::Error::Tls(_) => "falha no TLS".into(),
        tungstenite::Error::Url(_) => "endereço do túnel inválido".into(),
        tungstenite::Error::Protocol(p) => format!("protocolo: {p}"),
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => "conexão encerrada".into(),
        other => {
            let text = other.to_string();
            text.chars().take(120).collect()
        }
    }
}

enum Cmd {
    Data(Bytes),
    End,
}

struct Stream {
    tx: mpsc::Sender<Cmd>,
    task: JoinHandle<()>,
}

pub async fn session(dial: &Dial, connector: &Connector, report: &mut Reporter) -> Ended {
    let ended = |retry: Retry, greeted: bool| Ended { retry, greeted };
    let request = match request(dial) {
        Ok(request) => request,
        Err(reason) => return ended(Retry::backoff(reason), false),
    };
    let connect =
        tokio_tungstenite::connect_async_tls_with_config(request, Some(ws_config()), true, Some(connector.clone()));
    let (ws, _) = match timeout(CONNECT_TIMEOUT, connect).await {
        Err(_) => return ended(Retry::backoff("o AI Router não respondeu a tempo"), false),
        Ok(Err(tungstenite::Error::Http(response))) => {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let body = response.body().as_deref().unwrap_or(&[]);
            return ended(
                protocol::classify_http(response.status().as_u16(), body, retry_after.as_deref()),
                false,
            );
        }
        Ok(Err(error)) => return ended(Retry::backoff(short_error(&error)), false),
        Ok(Ok(pair)) => pair,
    };
    report.set_state(State::Connected, None);

    let (mut sink, mut source) = ws.split();
    let (out, mut out_rx) = mpsc::channel::<Message>(OUT_QUEUE);
    let mut writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            if sink.feed(message).await.is_err() {
                return;
            }
            while let Ok(message) = out_rx.try_recv() {
                if sink.feed(message).await.is_err() {
                    return;
                }
            }
            if sink.flush().await.is_err() {
                return;
            }
        }
    });

    let mut link = Link {
        out,
        streams: HashMap::new(),
        targets: &dial.targets,
        done: mpsc::unbounded_channel(),
        opened: 0,
    };
    let mut greeted = false;
    let mut pings = interval_at(Instant::now() + PING_EVERY, PING_EVERY);
    let mut ping_count: u32 = 0;
    let idle = sleep(IDLE_LIMIT);
    tokio::pin!(idle);

    let retry = loop {
        tokio::select! {
            message = source.next() => {
                idle.as_mut().reset(Instant::now() + IDLE_LIMIT);
                match message {
                    None => break Retry::backoff("conexão encerrada"),
                    Some(Err(error)) => break Retry::backoff(short_error(&error)),
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Err(retry) = link.on_binary(bytes).await {
                            break retry;
                        }
                    }
                    Some(Ok(Message::Text(text))) => match protocol::parse_control(text.as_str()) {
                        Control::Hello { person_name } => {
                            greeted = true;
                            let who = person_name.clone().unwrap_or_else(|| "?".into());
                            report.set_person(person_name);
                            report.log(&format!("conectado ({who})"));
                        }
                        Control::Egress { ip, at } => {
                            report.log(&format!("IP de saída medido pelo roteador: {ip}"));
                            report.set_egress(&ip, at);
                        }
                        Control::Other => {}
                    },
                    Some(Ok(Message::Close(frame))) => break protocol::classify_close(frame.map(|f| u16::from(f.code))),
                    Some(Ok(_)) => {}
                }
            }
            _ = pings.tick() => {
                if !link.emit(Message::Ping(Bytes::new())).await {
                    break Retry::backoff("o envio para o AI Router travou");
                }
                ping_count += 1;
                if ping_count.is_multiple_of(HEARTBEAT_PINGS) {
                    report.touch();
                }
            }
            () = &mut idle => break Retry::backoff("o AI Router parou de responder (60 s)"),
            Some(id) = link.done.1.recv() => {
                link.streams.remove(&id);
            }
            _ = &mut writer => break Retry::backoff("conexão encerrada"),
        }
    };

    for (_, stream) in link.streams.drain() {
        stream.task.abort();
    }
    if !writer.is_finished() {
        let _ = timeout(Duration::from_secs(2), link.out.send(Message::Close(None))).await;
        drop(link.out);
        let _ = timeout(Duration::from_secs(2), &mut writer).await;
        writer.abort();
    }
    if greeted {
        report.log(&format!(
            "desconectado: {} ({} conexões repassadas nesta sessão)",
            retry.reason, link.opened
        ));
    }
    ended(retry, greeted)
}

struct Link<'a> {
    out: mpsc::Sender<Message>,
    streams: HashMap<u32, Stream>,
    targets: &'a Targets,
    done: (mpsc::UnboundedSender<u32>, mpsc::UnboundedReceiver<u32>),
    opened: u64,
}

impl Link<'_> {
    async fn emit(&self, message: Message) -> bool {
        matches!(timeout(STALLED_SOCKET, self.out.send(message)).await, Ok(Ok(())))
    }

    async fn frame(&self, kind: u8, stream: u32, payload: &[u8]) -> Result<(), Retry> {
        if self
            .emit(Message::binary(protocol::encode(kind, stream, payload)))
            .await
        {
            Ok(())
        } else {
            Err(Retry::backoff("o envio para o AI Router travou"))
        }
    }

    async fn on_binary(&mut self, bytes: Bytes) -> Result<(), Retry> {
        let Some(frame) = protocol::parse(&bytes) else {
            let close = CloseFrame {
                code: CloseCode::Protocol,
                reason: "malformed frame".into(),
            };
            let _ = self.emit(Message::Close(Some(close))).await;
            return Err(Retry::backoff("o AI Router mandou um frame malformado"));
        };
        let id = frame.stream;
        match frame.kind {
            OPEN => self.open(id, frame.payload).await,
            DATA => {
                let Some(stream) = self.streams.get(&id) else {
                    return self.frame(RESET, id, b"unknown_stream").await;
                };
                let payload = bytes.slice(HEADER_LEN..);
                match timeout(STALLED_STREAM, stream.tx.send(Cmd::Data(payload))).await {
                    Ok(Ok(())) => Ok(()),
                    _ => {
                        self.drop_stream(id);
                        self.frame(RESET, id, b"stalled").await
                    }
                }
            }
            END => {
                if let Some(stream) = self.streams.get(&id)
                    && timeout(STALLED_STREAM, stream.tx.send(Cmd::End)).await.is_err()
                {
                    self.drop_stream(id);
                    return self.frame(RESET, id, b"stalled").await;
                }
                Ok(())
            }
            RESET => {
                self.drop_stream(id);
                Ok(())
            }
            _ if id == 0 => Ok(()),
            _ => {
                self.drop_stream(id);
                self.frame(RESET, id, b"unknown_frame").await
            }
        }
    }

    fn drop_stream(&mut self, id: u32) {
        if let Some(stream) = self.streams.remove(&id) {
            stream.task.abort();
        }
    }

    async fn open(&mut self, id: u32, payload: &[u8]) -> Result<(), Retry> {
        if id == 0 {
            return self.frame(REFUSED, id, b"not_allowed").await;
        }
        let target = std::str::from_utf8(payload).ok().and_then(|t| self.targets.resolve(t));
        let Some((host, port)) = target else {
            return self.frame(REFUSED, id, b"not_allowed").await;
        };
        if self.streams.contains_key(&id) {
            self.drop_stream(id);
            return self.frame(RESET, id, b"duplicate_stream").await;
        }
        if self.streams.len() >= MAX_STREAMS {
            return self.frame(REFUSED, id, b"too_many_streams").await;
        }
        let (tx, rx) = mpsc::channel(STREAM_QUEUE);
        let task = tokio::spawn(relay(id, host, port, rx, self.out.clone(), self.done.0.clone()));
        self.streams.insert(id, Stream { tx, task });
        self.opened += 1;
        Ok(())
    }
}

async fn dial(host: &str, port: u16) -> Result<TcpStream, &'static [u8]> {
    let deadline = Instant::now() + DIAL_TIMEOUT;
    let addrs: Vec<_> = match timeout_at(deadline, tokio::net::lookup_host((host, port))).await {
        Err(_) => return Err(b"timeout"),
        Ok(Err(_)) => return Err(b"resolve_failed"),
        Ok(Ok(addrs)) => addrs.collect(),
    };
    if addrs.is_empty() {
        return Err(b"resolve_failed");
    }
    for addr in addrs {
        match timeout_at(deadline, TcpStream::connect(addr)).await {
            Err(_) => return Err(b"timeout"),
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(_)) => continue,
        }
    }
    Err(b"connect_failed")
}

async fn relay(
    id: u32,
    host: String,
    port: u16,
    mut cmds: mpsc::Receiver<Cmd>,
    out: mpsc::Sender<Message>,
    done: mpsc::UnboundedSender<u32>,
) {
    let send = |kind: u8, payload: &[u8]| Message::binary(protocol::encode(kind, id, payload));
    let tcp = match dial(&host, port).await {
        Ok(tcp) => tcp,
        Err(reason) => {
            let _ = out.send(send(REFUSED, reason)).await;
            let _ = done.send(id);
            return;
        }
    };
    let _ = tcp.set_nodelay(true);
    if out.send(send(OPENED, &[])).await.is_err() {
        return;
    }
    let (mut reader, mut writer) = tcp.into_split();
    let upstream = {
        let out = out.clone();
        async move {
            let mut buf = vec![0u8; MAX_DATA];
            loop {
                let n = reader.read(&mut buf).await.map_err(|_| "read_failed")?;
                if n == 0 {
                    out.send(send(END, &[])).await.map_err(|_| "closed")?;
                    return Ok::<(), &'static str>(());
                }
                out.send(send(DATA, &buf[..n])).await.map_err(|_| "closed")?;
            }
        }
    };
    let downstream = async move {
        while let Some(cmd) = cmds.recv().await {
            match cmd {
                Cmd::Data(bytes) => writer.write_all(&bytes).await.map_err(|_| "write_failed")?,
                Cmd::End => {
                    let _ = writer.shutdown().await;
                    return Ok(());
                }
            }
        }
        Err("closed")
    };
    if let Err(reason) = tokio::try_join!(upstream, downstream)
        && reason != "closed"
    {
        let _ = out.send(send(RESET, reason.as_bytes())).await;
    }
    let _ = done.send(id);
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
    use tokio_tungstenite::tungstenite::http::StatusCode;

    use super::*;
    use crate::testutil::temp_dir;

    const KEY: &str = "sk-etd-test-key";

    async fn echo_server() -> (String, mpsc::UnboundedReceiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (closed_tx, closed_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let closed = closed_tx.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    loop {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if socket.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    let _ = socket.shutdown().await;
                    let _ = closed.send(());
                });
            }
        });
        (addr, closed_rx)
    }

    type Ws = WebSocketStream<TcpStream>;

    async fn router(refuse: Option<(StatusCode, &'static str)>) -> (String, tokio::sync::oneshot::Receiver<Ws>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}{}", listener.local_addr().unwrap(), protocol::PATH);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let check = move |request: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
                let header = |name: &str| {
                    request
                        .headers()
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string()
                };
                assert_eq!(request.uri().path(), protocol::PATH);
                assert_eq!(header("authorization"), format!("Bearer {KEY}"));
                assert_eq!(header("sec-websocket-protocol"), protocol::SUBPROTOCOL);
                assert!(header("user-agent").starts_with("etus-cli/"));
                assert_eq!(header("x-etus-device-name"), "mac-teste");
                if let Some((status, body)) = refuse {
                    let mut error = ErrorResponse::new(Some(body.to_string()));
                    *error.status_mut() = status;
                    return Err(error);
                }
                response
                    .headers_mut()
                    .insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(protocol::SUBPROTOCOL));
                Ok(response)
            };
            if let Ok(ws) = tokio_tungstenite::accept_hdr_async(socket, check).await {
                let _ = tx.send(ws);
            }
        });
        (url, rx)
    }

    fn dial(url: String, extra: Option<String>) -> Dial {
        Dial {
            url,
            key: KEY.into(),
            device_name: "mac-teste".into(),
            targets: Targets { extra },
        }
    }

    async fn send(ws: &mut Ws, kind: u8, stream: u32, payload: &[u8]) {
        ws.send(Message::binary(protocol::encode(kind, stream, payload)))
            .await
            .unwrap();
    }

    async fn next_frame(ws: &mut Ws) -> (u8, u32, Vec<u8>) {
        loop {
            let message = timeout(Duration::from_secs(10), ws.next())
                .await
                .expect("frame in time")
                .expect("socket open")
                .unwrap();
            if let Message::Binary(bytes) = message {
                let frame = protocol::parse(&bytes).expect("well formed");
                return (frame.kind, frame.stream, frame.payload.to_vec());
            }
        }
    }

    async fn echo_round_trip(ws: &mut Ws, stream: u32, payload: &[u8]) {
        for chunk in payload.chunks(MAX_DATA) {
            send(ws, DATA, stream, chunk).await;
        }
        let mut got = Vec::new();
        while got.len() < payload.len() {
            let (kind, id, bytes) = next_frame(ws).await;
            assert_eq!((kind, id), (DATA, stream));
            assert!(bytes.len() <= MAX_DATA);
            got.extend_from_slice(&bytes);
        }
        assert_eq!(got, payload);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn relays_bytes_end_and_reset_and_refuses_other_hosts() {
        let (echo, mut closed) = echo_server().await;
        let (url, accepted) = router(None).await;
        let dir = temp_dir("egress-tunnel");
        let mut report = Reporter::new(&dir, false);
        let dial = dial(url, Some(echo.clone()));
        let connector = tls_connector();

        let server = async move {
            let mut ws = accepted.await.unwrap();
            ws.send(Message::text(
                r#"{"type":"hello","personName":"Ana","hosts":["api.anthropic.com:443"],"pingSeconds":20}"#,
            ))
            .await
            .unwrap();
            ws.send(Message::text(
                r#"{"type":"egress","ip":"203.0.113.7","at":"2026-10-17T12:00:00.000Z"}"#,
            ))
            .await
            .unwrap();

            for (id, target) in [
                (1, "evil.example.com:443"),
                (3, "api.anthropic.com:8443"),
                (5, "API.anthropic.com:443"),
            ] {
                send(&mut ws, OPEN, id, target.as_bytes()).await;
                assert_eq!(next_frame(&mut ws).await, (REFUSED, id, b"not_allowed".to_vec()));
            }

            send(&mut ws, OPEN, 7, echo.as_bytes()).await;
            assert_eq!(next_frame(&mut ws).await, (OPENED, 7, Vec::new()));
            echo_round_trip(&mut ws, 7, b"hello").await;
            send(&mut ws, END, 7, &[]).await;
            assert_eq!(next_frame(&mut ws).await, (END, 7, Vec::new()));
            timeout(Duration::from_secs(5), closed.recv()).await.unwrap().unwrap();

            send(&mut ws, OPEN, 9, echo.as_bytes()).await;
            assert_eq!(next_frame(&mut ws).await, (OPENED, 9, Vec::new()));
            let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
            echo_round_trip(&mut ws, 9, &big).await;
            send(&mut ws, RESET, 9, b"bye").await;
            timeout(Duration::from_secs(5), closed.recv()).await.unwrap().unwrap();

            send(&mut ws, DATA, 9, b"late").await;
            assert_eq!(next_frame(&mut ws).await, (RESET, 9, b"unknown_stream".to_vec()));
            send(&mut ws, 0x7f, 0, b"").await;
            send(&mut ws, 0x7f, 11, b"").await;
            assert_eq!(next_frame(&mut ws).await, (RESET, 11, b"unknown_frame".to_vec()));

            ws.close(Some(CloseFrame {
                code: CloseCode::from(4002),
                reason: "replaced".into(),
            }))
            .await
            .unwrap();
            while let Some(Ok(_)) = ws.next().await {}
        };

        let (ended, ()) = tokio::join!(session(&dial, &connector, &mut report), server);
        assert!(ended.greeted);
        assert_eq!(ended.retry.state, State::Replaced);
        let snapshot = report.snapshot();
        assert_eq!(snapshot.person_name.as_deref(), Some("Ana"));
        assert_eq!(snapshot.egress_ip.as_deref(), Some("203.0.113.7"));
        let log = std::fs::read_to_string(super::super::state::log_path(&dir)).unwrap();
        assert!(log.contains("conectado (Ana)"));
        assert!(log.contains("203.0.113.7"));
        assert!(!log.contains(KEY));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refusal_before_upgrade_waits_for_the_admin() {
        let (url, _accepted) = router(Some((
            StatusCode::FORBIDDEN,
            r#"{"error":{"code":"home_egress_off","message":"off"}}"#,
        )))
        .await;
        let dir = temp_dir("egress-refused");
        let mut report = Reporter::new(&dir, false);
        let ended = session(&dial(url, None), &tls_connector(), &mut report).await;
        assert!(!ended.greeted);
        assert_eq!(ended.retry.state, State::WaitingAdmin);
        assert_eq!(ended.retry.wait, protocol::Wait::Fixed(Duration::from_secs(300)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn malformed_frame_closes_with_1002() {
        let (url, accepted) = router(None).await;
        let dir = temp_dir("egress-malformed");
        let mut report = Reporter::new(&dir, false);
        let dial = dial(url, None);
        let connector = tls_connector();
        let server = async move {
            let mut ws = accepted.await.unwrap();
            ws.send(Message::binary(vec![OPEN, 0, 0])).await.unwrap();
            loop {
                match timeout(Duration::from_secs(10), ws.next()).await.unwrap() {
                    Some(Ok(Message::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                    Some(Ok(_)) => continue,
                    _ => return None,
                }
            }
        };
        let (ended, code) = tokio::join!(session(&dial, &connector, &mut report), server);
        assert_eq!(code, Some(1002));
        assert_eq!(ended.retry.state, State::Offline);
    }
}
