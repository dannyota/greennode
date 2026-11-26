use std::collections::VecDeque;
use std::net::{SocketAddr, TcpListener};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio_rustls::{
    TlsAcceptor,
    rustls::{ServerConfig, pki_types::PrivatePkcs8KeyDer},
};

pub enum Response {
    Http {
        status: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    Raw(Vec<u8>),
    RawClose(Vec<u8>),
    Close,
    ResetMidBody(oneshot::Sender<()>),
    Stall(oneshot::Sender<()>, usize),
    StallBeforeHead(oneshot::Sender<()>),
    ObserveStall {
        head: Vec<u8>,
        sent: oneshot::Sender<()>,
        eof: oneshot::Sender<bool>,
    },
    Fragment {
        first: oneshot::Sender<()>,
        proceed: oneshot::Receiver<()>,
        last: oneshot::Sender<bool>,
    },
}

pub struct Server {
    pub root_der: rustls::pki_types::CertificateDer<'static>,
    pub origin: String,
    pub address: SocketAddr,
    #[cfg(feature = "test-support")]
    fixture: (SocketAddr, rustls::pki_types::CertificateDer<'static>),
    requests: Arc<AtomicUsize>,
    connections: Arc<AtomicUsize>,
    protocol: Arc<AtomicUsize>,
    heads: Arc<Mutex<Vec<Vec<u8>>>>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["localhost".into(), address.ip().to_string()])
                .unwrap();
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
            )
            .unwrap();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let root_der = cert.der().clone();
        let requests = Arc::new(AtomicUsize::new(0));
        let connections = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::clone(&requests);
        let connection_count = Arc::clone(&connections);
        let protocol = Arc::new(AtomicUsize::new(0));
        let protocol_count = Arc::clone(&protocol);
        let heads = Arc::new(Mutex::new(Vec::new()));
        let request_heads = Arc::clone(&heads);
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let request_bodies = Arc::clone(&bodies);
        let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
        let (shutdown, mut stop) = oneshot::channel();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let acceptor = TlsAcceptor::from(Arc::new(config));
                let mut tasks = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        _ = &mut stop => break,
                        result = listener.accept() => {
                            let (stream, _) = result.unwrap();
                            connection_count.fetch_add(1, Ordering::SeqCst);
                            let acceptor = acceptor.clone();
                            let responses = Arc::clone(&responses);
                            let requests = Arc::clone(&request_count);
                            let protocol = Arc::clone(&protocol_count);
                            let request_heads = Arc::clone(&request_heads);
                            let request_bodies = Arc::clone(&request_bodies);
                            tasks.spawn(async move {
                                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async move {
                                let Ok(mut stream) = acceptor.accept(stream).await else { return };
                                let negotiated = match stream.get_ref().1.alpn_protocol() { Some(b"h2") => 3, Some(b"http/1.1") => 2, _ => 1 };
                                protocol.store(negotiated, Ordering::SeqCst);
                                loop {
                                    let mut head = Vec::new();
                                    while !head.ends_with(b"\r\n\r\n") {
                                        let mut byte = [0];
                                        if stream.read_exact(&mut byte).await.is_err() { return; }
                                        head.push(byte[0]);
                                        if head.len() > 65536 { return; }
                                    }
                                    request_heads.lock().unwrap().push(head.clone());
                                    let text = String::from_utf8_lossy(&head);
                                    let length = text.lines().filter_map(|line| line.split_once(':')).find_map(|(key, value)| {
                                        if key.eq_ignore_ascii_case("content-length") { value.trim().parse::<usize>().ok() } else { None }
                                    }).unwrap_or(0);
                                    if length > 8 * 1024 * 1024 { return; }
                                    let response = responses.lock().unwrap().pop_front();
                                    let Some(response) = response else { return; };
                                    let response = match response {
                                        Response::ResetMidBody(notify) => {
                                            let mut byte = [0];
                                            if stream.read_exact(&mut byte).await.is_err() { return; }
                                            if stream.get_ref().0.set_zero_linger().is_err() { return; }
                                            let _ = notify.send(());
                                            return;
                                        }
                                        response => response,
                                    };
                                    let mut body = vec![0; length];
                                    if stream.read_exact(&mut body).await.is_err() { return; }
                                    request_bodies.lock().unwrap().push(body);
                                    requests.fetch_add(1, Ordering::SeqCst);
                                    let bytes = match response {
                                        Response::ResetMidBody(_) => unreachable!(),
                                        Response::Fragment { first, proceed, last } => {
                                            if stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nTransfer-Encoding: chunked\r\nTrailer: X-Trailer\r\n\r\n3\r\nabc\r\n").await.is_err() || stream.flush().await.is_err() { return; }
                                            let _ = first.send(());
                                            if proceed.await.is_err() { return; }
                                            let written = stream.write_all(b"1\r\nd\r\n0\r\nX-Trailer: done\r\n\r\n").await.is_ok() && stream.flush().await.is_ok();
                                            let _ = last.send(written);
                                            return;
                                        }
                                        Response::ObserveStall { head, sent, eof } => {
                                            if stream.write_all(&head).await.is_err() || stream.flush().await.is_err() { return; }
                                            let _ = sent.send(());
                                            let mut byte = [0];
                                            let result = stream.read(&mut byte).await;
                                            let _ = eof.send(matches!(result, Ok(0)) || matches!(result, Err(ref e) if matches!(e.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset)));
                                            return;
                                        }
                                        Response::Close => return,
                                        Response::StallBeforeHead(started) => {
                                            let _ = started.send(());
                                            std::future::pending::<()>().await;
                                            return;
                                        }
                                        Response::Stall(started, length) => {
                                            if stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n").as_bytes()).await.is_err() || stream.flush().await.is_err() { return; }
                                            let _ = started.send(());
                                            std::future::pending::<()>().await;
                                            return;
                                        }
                                        Response::RawClose(bytes) => {
                                            let _ = stream.write_all(&bytes).await;
                                            let _ = stream.flush().await;
                                            return;
                                        }
                                        Response::Raw(bytes) => bytes,
                                        Response::Http { status, headers, body } => {
                                            let mut bytes = format!("HTTP/1.1 {status}\r\n").into_bytes();
                                            for (name, value) in headers { bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes()); }
                                            bytes.extend_from_slice(b"\r\n");
                                            bytes.extend_from_slice(&body);
                                            bytes
                                        }
                                    };
                                    if stream.write_all(&bytes).await.is_err() || stream.flush().await.is_err() { return; }
                                }
                                }).await;
                            });
                        }
                    }
                }
                tasks.shutdown().await;
            });
        });
        Self {
            #[cfg(feature = "test-support")]
            fixture: (address, root_der.clone()),
            root_der,
            origin: format!("https://localhost:{}", address.port()),
            address,
            requests,
            connections,
            protocol,
            heads,
            bodies,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }
    #[cfg(feature = "test-support")]
    pub(crate) fn fixture(&self) -> (SocketAddr, &rustls::pki_types::CertificateDer<'static>) {
        (self.fixture.0, &self.fixture.1)
    }
    pub fn heads(&self) -> Vec<Vec<u8>> {
        self.heads.lock().unwrap().clone()
    }
    pub fn bodies(&self) -> Vec<Vec<u8>> {
        self.bodies.lock().unwrap().clone()
    }
    pub fn protocol(&self) -> usize {
        self.protocol.load(Ordering::SeqCst)
    }
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.shutdown.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
