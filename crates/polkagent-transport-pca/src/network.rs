//! Durable, TCP-backed PCA peer transport.
//!
//! [`TcpPcaTransport`] is the first PCA adapter in this crate that moves
//! messages across an operating-system network socket. It deliberately keeps
//! two acknowledgement boundaries separate:
//!
//! 1. A wire ACK is emitted only after the receiver atomically persists the
//!    delivery and its deduplication marker.
//! 2. [`Transport::ack`] removes that persisted delivery only after the
//!    application has durably admitted it.
//!
//! The sender persists an outbox entry before [`Transport::send`] returns. A
//! background worker reconnects and retries the oldest entry until it receives
//! the receiver's durable wire ACK. If the connection fails after persistence
//! but before the ACK arrives, the receiver recognizes the repeated delivery
//! ID and ACKs it without enqueueing a second application message.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use polkagent_core::{now, ConversationId};
use polkagent_transport_trait::{
    AuthenticatedSender, DeliveryId, DeliveryReceipt, IncomingMessage, MessageBody, OutgoingBody,
    OutgoingMessage, SenderTrustTier, Transport, TransportCapabilities, TransportError, UserId,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::PcaError;
use crate::session::{EncryptedEnvelope, Session};
use crate::PcaWireMessage;

const PROTOCOL_VERSION: u16 = 1;
const STATE_VERSION: u16 = 1;
const FRAME_OVERHEAD_BYTES: u64 = 64 * 1024;

/// A TCP peer and the identity expected during the encrypted handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpPeer {
    /// Peer TCP socket address.
    pub socket_addr: SocketAddr,
    /// SS58 identity the peer must advertise.
    pub ss58_address: String,
}

impl TcpPeer {
    /// Create a peer configuration.
    pub fn new(socket_addr: SocketAddr, ss58_address: impl Into<String>) -> Self {
        Self {
            socket_addr,
            ss58_address: ss58_address.into(),
        }
    }
}

/// Configuration for [`TcpPcaTransport`].
#[derive(Debug, Clone)]
pub struct TcpPcaConfig {
    /// Local identity advertised during the handshake and placed on messages.
    pub local_ss58_address: String,
    /// Address on which to accept peer connections. Port zero is supported.
    pub listen_addr: SocketAddr,
    /// Optional outbound peer. A receive-only node may omit it.
    pub peer: Option<TcpPeer>,
    /// File holding the crash-safe inbox, outbox, and dedup state.
    pub state_path: PathBuf,
    /// Maximum persisted inbox or outbox depth.
    pub max_in_flight: usize,
    /// Maximum decrypted PCA wire-message size.
    pub max_message_bytes: u64,
    /// Delay between reconnect attempts while the durable outbox is non-empty.
    pub retry_interval: Duration,
    /// Timeout for TCP connection and peer wire acknowledgements.
    pub io_timeout: Duration,
    /// Time after which an application delivery without [`Transport::ack`] is
    /// eligible for at-least-once redelivery within the same process.
    pub application_lease_timeout: Duration,
    /// Encrypted session inactivity timeout.
    pub session_timeout: Duration,
}

impl TcpPcaConfig {
    /// Create a configuration with conservative local defaults.
    pub fn new(
        local_ss58_address: impl Into<String>,
        listen_addr: SocketAddr,
        state_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            local_ss58_address: local_ss58_address.into(),
            listen_addr,
            peer: None,
            state_path: state_path.into(),
            max_in_flight: 1_024,
            max_message_bytes: 1024 * 1024,
            retry_interval: Duration::from_millis(250),
            io_timeout: Duration::from_secs(5),
            application_lease_timeout: Duration::from_secs(30),
            session_timeout: Duration::from_secs(300),
        }
    }

    /// Configure the outbound peer.
    #[must_use]
    pub fn with_peer(mut self, peer: TcpPeer) -> Self {
        self.peer = Some(peer);
        self
    }

    /// Configure the retry interval.
    #[must_use]
    pub fn with_retry_interval(mut self, interval: Duration) -> Self {
        self.retry_interval = interval;
        self
    }

    /// Configure the I/O timeout.
    #[must_use]
    pub fn with_io_timeout(mut self, timeout: Duration) -> Self {
        self.io_timeout = timeout;
        self
    }

    /// Configure the application delivery lease timeout.
    #[must_use]
    pub fn with_application_lease_timeout(mut self, timeout: Duration) -> Self {
        self.application_lease_timeout = timeout;
        self
    }

    /// Configure the maximum message size.
    #[must_use]
    pub fn with_max_message_bytes(mut self, max: u64) -> Self {
        self.max_message_bytes = max;
        self
    }

    /// Configure the maximum inbox/outbox depth.
    #[must_use]
    pub fn with_max_in_flight(mut self, max: usize) -> Self {
        self.max_in_flight = max;
        self
    }

    fn validate(&self) -> Result<(), PcaError> {
        if self.local_ss58_address.is_empty() {
            return Err(PcaError::ConfigError {
                reason: "local_ss58_address must not be empty".into(),
            });
        }
        if self.max_in_flight == 0 {
            return Err(PcaError::ConfigError {
                reason: "max_in_flight must be greater than zero".into(),
            });
        }
        if self.max_message_bytes == 0 {
            return Err(PcaError::ConfigError {
                reason: "max_message_bytes must be greater than zero".into(),
            });
        }
        if self.retry_interval.is_zero()
            || self.io_timeout.is_zero()
            || self.application_lease_timeout.is_zero()
        {
            return Err(PcaError::ConfigError {
                reason: "retry_interval, io_timeout, and application_lease_timeout must be greater than zero".into(),
            });
        }
        if let Some(peer) = &self.peer {
            if peer.ss58_address.is_empty() {
                return Err(PcaError::ConfigError {
                    reason: "peer SS58 address must not be empty".into(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DurableDelivery {
    delivery_id: String,
    payload: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkState {
    version: u16,
    inbox: Vec<DurableDelivery>,
    outbox: Vec<DurableDelivery>,
    dedup_markers: BTreeSet<String>,
}

impl Default for NetworkState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            inbox: Vec::new(),
            outbox: Vec::new(),
            dedup_markers: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct StateFile {
    path: PathBuf,
}

impl StateFile {
    fn open(path: PathBuf) -> Result<(Self, NetworkState), PcaError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| PcaError::PersistenceError {
                reason: format!("failed to create {}: {error}", parent.display()),
            })?;
        }

        let store = Self { path };
        let state = if store.path.exists() {
            let bytes = fs::read(&store.path).map_err(|error| PcaError::PersistenceError {
                reason: format!("failed to read {}: {error}", store.path.display()),
            })?;
            let loaded: NetworkState =
                serde_json::from_slice(&bytes).map_err(|error| PcaError::PersistenceError {
                    reason: format!("failed to decode {}: {error}", store.path.display()),
                })?;
            if loaded.version != STATE_VERSION {
                return Err(PcaError::PersistenceError {
                    reason: format!(
                        "unsupported TCP PCA state version {} (expected {STATE_VERSION})",
                        loaded.version
                    ),
                });
            }
            loaded
        } else {
            NetworkState::default()
        };
        Ok((store, state))
    }

    /// Atomically replace and fsync the state file before returning.
    fn commit(&self, state: &NetworkState) -> Result<(), PcaError> {
        let bytes =
            serde_json::to_vec_pretty(state).map_err(|error| PcaError::PersistenceError {
                reason: format!("failed to encode TCP PCA state: {error}"),
            })?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let temp_path = parent.join(format!(".pca-state-{}.tmp", Uuid::now_v7()));
        let result = (|| -> Result<(), std::io::Error> {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp_path)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temp_path, &self.path)?;
            if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();

        if let Err(error) = result {
            let _ = fs::remove_file(&temp_path);
            return Err(PcaError::PersistenceError {
                reason: format!("failed to commit {}: {error}", self.path.display()),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireFrame {
    Hello {
        protocol_version: u16,
        ss58_address: String,
        public_key: [u8; 32],
    },
    HelloAck {
        protocol_version: u16,
        ss58_address: String,
        public_key: [u8; 32],
    },
    Data {
        delivery_id: String,
        envelope: EncryptedEnvelope,
    },
    Ack {
        delivery_id: String,
    },
    Error {
        message: String,
    },
}

/// A cross-process TCP PCA transport with a durable inbox and outbox.
pub struct TcpPcaTransport {
    config: TcpPcaConfig,
    listen_addr: SocketAddr,
    peer: RwLock<Option<TcpPeer>>,
    state_file: StateFile,
    state: tokio::sync::Mutex<NetworkState>,
    leased: Mutex<BTreeMap<String, Instant>>,
    inbox_notify: Arc<Notify>,
    outbox_notify: Arc<Notify>,
    shutdown_notify: Arc<Notify>,
    shutdown: Arc<AtomicBool>,
    peer_connected: AtomicBool,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl TcpPcaTransport {
    /// Bind the configured listener, load durable state, and start reconnect
    /// workers. Pending inbox entries are immediately available for recovery.
    pub async fn bind(config: TcpPcaConfig) -> Result<Arc<Self>, PcaError> {
        config.validate()?;
        let listener = TcpListener::bind(config.listen_addr)
            .await
            .map_err(|error| PcaError::NetworkError {
                reason: format!("failed to bind {}: {error}", config.listen_addr),
            })?;
        let listen_addr = listener
            .local_addr()
            .map_err(|error| PcaError::NetworkError {
                reason: format!("failed to inspect listener: {error}"),
            })?;
        let (state_file, state) = StateFile::open(config.state_path.clone())?;
        if state.inbox.len() > config.max_in_flight || state.outbox.len() > config.max_in_flight {
            return Err(PcaError::PersistenceError {
                reason: "persisted queue exceeds configured max_in_flight".into(),
            });
        }

        let transport = Arc::new(Self {
            peer: RwLock::new(config.peer.clone()),
            config,
            listen_addr,
            state_file,
            state: tokio::sync::Mutex::new(state),
            leased: Mutex::new(BTreeMap::new()),
            inbox_notify: Arc::new(Notify::new()),
            outbox_notify: Arc::new(Notify::new()),
            shutdown_notify: Arc::new(Notify::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            peer_connected: AtomicBool::new(false),
            tasks: Mutex::new(Vec::new()),
        });

        let listener_transport = Arc::downgrade(&transport);
        let listener_shutdown = Arc::clone(&transport.shutdown_notify);
        let listener_task = tokio::spawn(async move {
            Self::listener_loop(listener_transport, listener, listener_shutdown).await;
        });
        let sender_transport = Arc::downgrade(&transport);
        let sender_shutdown = Arc::clone(&transport.shutdown);
        let sender_shutdown_notify = Arc::clone(&transport.shutdown_notify);
        let sender_outbox_notify = Arc::clone(&transport.outbox_notify);
        let sender_task = tokio::spawn(async move {
            Self::sender_loop(
                sender_transport,
                sender_shutdown,
                sender_shutdown_notify,
                sender_outbox_notify,
            )
            .await;
        });
        transport.tasks.lock().extend([listener_task, sender_task]);
        transport.inbox_notify.notify_waiters();
        transport.outbox_notify.notify_one();
        info!(address = %listen_addr, "TCP PCA transport listening");
        Ok(transport)
    }

    /// Return the actual listener address, including an OS-selected port.
    pub fn listen_addr(&self) -> SocketAddr {
        self.listen_addr
    }

    /// Change the outbound peer and wake the reconnect worker.
    pub async fn set_peer(&self, peer: TcpPeer) {
        *self.peer.write().await = Some(peer);
        self.outbox_notify.notify_one();
    }

    /// Return whether an outbound peer connection is currently active.
    pub fn is_peer_connected(&self) -> bool {
        self.peer_connected.load(Ordering::SeqCst)
    }

    /// Return the number of persisted, application-unacknowledged deliveries.
    pub async fn pending_inbox_count(&self) -> usize {
        self.state.lock().await.inbox.len()
    }

    /// Return the number of persisted messages awaiting a peer wire ACK.
    pub async fn pending_outbox_count(&self) -> usize {
        self.state.lock().await.outbox.len()
    }

    /// Return the number of durable deduplication markers.
    pub async fn dedup_marker_count(&self) -> usize {
        self.state.lock().await.dedup_markers.len()
    }

    /// Stop listener/reconnect tasks and release the bound socket.
    pub async fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shutdown_notify.notify_waiters();
        self.inbox_notify.notify_waiters();
        self.outbox_notify.notify_waiters();
        let tasks = std::mem::take(&mut *self.tasks.lock());
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
        self.peer_connected.store(false, Ordering::SeqCst);
    }

    async fn listener_loop(
        transport: Weak<Self>,
        listener: TcpListener,
        shutdown_notify: Arc<Notify>,
    ) {
        loop {
            tokio::select! {
                () = shutdown_notify.notified() => return,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, remote)) => {
                            let connection_transport = Weak::clone(&transport);
                            tokio::spawn(async move {
                                if let Err(error) = Self::handle_connection(connection_transport, stream).await {
                                    debug!(%remote, %error, "TCP PCA peer connection ended");
                                }
                            });
                        }
                        Err(error) => warn!(%error, "TCP PCA accept failed"),
                    }
                }
            }
        }
    }

    async fn handle_connection(
        transport: Weak<Self>,
        mut stream: TcpStream,
    ) -> Result<(), PcaError> {
        let current = transport.upgrade().ok_or(PcaError::Shutdown)?;
        let max_frame_bytes = current.max_frame_bytes();
        let max_message_bytes = current.config.max_message_bytes;
        let local_address = current.config.local_ss58_address.clone();
        let session_timeout = current.config.session_timeout;
        let shutdown = Arc::clone(&current.shutdown);
        let shutdown_notify = Arc::clone(&current.shutdown_notify);
        drop(current);

        let hello =
            read_frame_until_shutdown(&mut stream, max_frame_bytes, &shutdown, &shutdown_notify)
                .await?;
        let (peer_address, peer_public_key) = match hello {
            WireFrame::Hello {
                protocol_version,
                ss58_address,
                public_key,
            } if protocol_version == PROTOCOL_VERSION => (ss58_address, public_key),
            WireFrame::Hello {
                protocol_version, ..
            } => {
                return Err(PcaError::ProtocolError {
                    reason: format!("unsupported protocol version {protocol_version}"),
                });
            }
            _ => {
                return Err(PcaError::ProtocolError {
                    reason: "first frame must be hello".into(),
                });
            }
        };

        let current = transport.upgrade().ok_or(PcaError::Shutdown)?;
        let expected_peer = current.peer.read().await.clone();
        drop(current);
        if let Some(expected) = expected_peer.as_ref() {
            if expected.ss58_address != peer_address {
                let _ = write_frame(
                    &mut stream,
                    &WireFrame::Error {
                        message: "peer identity mismatch".into(),
                    },
                    max_frame_bytes,
                )
                .await;
                return Err(PcaError::PeerAuthenticationFailed {
                    peer_address,
                    reason: "advertised identity does not match configured peer".into(),
                });
            }
        }

        let (mut session, local_public_key) = Session::accept(
            local_address.clone(),
            &peer_public_key,
            peer_address.clone(),
            session_timeout,
        )?;
        write_frame(
            &mut stream,
            &WireFrame::HelloAck {
                protocol_version: PROTOCOL_VERSION,
                ss58_address: local_address,
                public_key: local_public_key,
            },
            max_frame_bytes,
        )
        .await?;

        loop {
            let frame = read_frame_until_shutdown(
                &mut stream,
                max_frame_bytes,
                &shutdown,
                &shutdown_notify,
            )
            .await?;
            let WireFrame::Data {
                delivery_id,
                envelope,
            } = frame
            else {
                return Err(PcaError::ProtocolError {
                    reason: "expected encrypted data frame".into(),
                });
            };
            let payload = session.decrypt(&envelope)?;
            if payload.len() as u64 > max_message_bytes {
                return Err(PcaError::ProtocolError {
                    reason: format!(
                        "decrypted message size {} exceeds maximum {}",
                        payload.len(),
                        max_message_bytes
                    ),
                });
            }
            validate_wire_payload(&payload, &peer_address)?;

            // This commit is the network ACK boundary. Duplicates are ACKed
            // again, but are never appended to the application inbox twice.
            let current = transport.upgrade().ok_or(PcaError::Shutdown)?;
            current.persist_inbound(&delivery_id, payload).await?;
            drop(current);
            write_frame(
                &mut stream,
                &WireFrame::Ack {
                    delivery_id: delivery_id.clone(),
                },
                max_frame_bytes,
            )
            .await?;
        }
    }

    async fn persist_inbound(&self, delivery_id: &str, payload: Vec<u8>) -> Result<bool, PcaError> {
        let mut state = self.state.lock().await;
        if state.dedup_markers.contains(delivery_id) {
            return Ok(false);
        }
        if state.inbox.len() >= self.config.max_in_flight {
            return Err(PcaError::ChannelError {
                reason: format!(
                    "durable inbox full ({} / {})",
                    state.inbox.len(),
                    self.config.max_in_flight
                ),
            });
        }
        let mut next = state.clone();
        next.dedup_markers.insert(delivery_id.to_owned());
        next.inbox.push(DurableDelivery {
            delivery_id: delivery_id.to_owned(),
            payload,
        });
        self.state_file.commit(&next)?;
        *state = next;
        drop(state);
        self.inbox_notify.notify_one();
        Ok(true)
    }

    async fn sender_loop(
        transport: Weak<Self>,
        shutdown: Arc<AtomicBool>,
        shutdown_notify: Arc<Notify>,
        outbox_notify: Arc<Notify>,
    ) {
        loop {
            if shutdown.load(Ordering::SeqCst) {
                return;
            }
            let Some(current) = transport.upgrade() else {
                return;
            };
            let has_work = !current.state.lock().await.outbox.is_empty();
            let retry_interval = current.config.retry_interval;
            if has_work {
                if let Err(error) = current.flush_outbox().await {
                    debug!(%error, "TCP PCA outbox flush will retry");
                }
                current.peer_connected.store(false, Ordering::SeqCst);
                drop(current);
                tokio::select! {
                    () = shutdown_notify.notified() => return,
                    () = outbox_notify.notified() => {},
                    () = tokio::time::sleep(retry_interval) => {},
                }
            } else {
                drop(current);
                tokio::select! {
                    () = shutdown_notify.notified() => return,
                    () = outbox_notify.notified() => {},
                }
            }
        }
    }

    async fn flush_outbox(&self) -> Result<(), PcaError> {
        let peer = self
            .peer
            .read()
            .await
            .clone()
            .ok_or_else(|| PcaError::ConfigError {
                reason: "outbox has work but no outbound peer is configured".into(),
            })?;
        let mut stream =
            tokio::time::timeout(self.config.io_timeout, TcpStream::connect(peer.socket_addr))
                .await
                .map_err(|_| PcaError::NetworkError {
                    reason: format!("connect to {} timed out", peer.socket_addr),
                })?
                .map_err(|error| PcaError::NetworkError {
                    reason: format!("connect to {} failed: {error}", peer.socket_addr),
                })?;
        let (mut session, local_public_key) = Session::initiate(
            self.config.local_ss58_address.clone(),
            self.config.session_timeout,
        );
        write_frame(
            &mut stream,
            &WireFrame::Hello {
                protocol_version: PROTOCOL_VERSION,
                ss58_address: self.config.local_ss58_address.clone(),
                public_key: local_public_key,
            },
            self.max_frame_bytes(),
        )
        .await?;
        let response = tokio::time::timeout(
            self.config.io_timeout,
            read_frame(&mut stream, self.max_frame_bytes()),
        )
        .await
        .map_err(|_| PcaError::NetworkError {
            reason: "peer handshake timed out".into(),
        })??;
        let peer_public_key = match response {
            WireFrame::HelloAck {
                protocol_version,
                ss58_address,
                public_key,
            } if protocol_version == PROTOCOL_VERSION && ss58_address == peer.ss58_address => {
                public_key
            }
            WireFrame::Error { message } => {
                return Err(PcaError::ProtocolError { reason: message })
            }
            _ => {
                return Err(PcaError::PeerAuthenticationFailed {
                    peer_address: peer.ss58_address,
                    reason: "invalid hello acknowledgement or identity mismatch".into(),
                });
            }
        };
        session.complete_handshake(&peer_public_key, peer.ss58_address)?;
        self.peer_connected.store(true, Ordering::SeqCst);

        loop {
            let next = self.state.lock().await.outbox.first().cloned();
            let Some(delivery) = next else {
                return Ok(());
            };
            let envelope = session.encrypt(&delivery.payload)?;
            write_frame(
                &mut stream,
                &WireFrame::Data {
                    delivery_id: delivery.delivery_id.clone(),
                    envelope,
                },
                self.max_frame_bytes(),
            )
            .await?;
            let ack = tokio::time::timeout(
                self.config.io_timeout,
                read_frame(&mut stream, self.max_frame_bytes()),
            )
            .await
            .map_err(|_| PcaError::NetworkError {
                reason: format!("wire ACK timed out for {}", delivery.delivery_id),
            })??;
            match ack {
                WireFrame::Ack { delivery_id } if delivery_id == delivery.delivery_id => {
                    self.remove_outbox(&delivery_id).await?;
                }
                WireFrame::Error { message } => {
                    return Err(PcaError::ProtocolError { reason: message });
                }
                _ => {
                    return Err(PcaError::ProtocolError {
                        reason: format!("unexpected ACK for {}", delivery.delivery_id),
                    });
                }
            }
        }
    }

    async fn remove_outbox(&self, delivery_id: &str) -> Result<(), PcaError> {
        let mut state = self.state.lock().await;
        let Some(position) = state
            .outbox
            .iter()
            .position(|entry| entry.delivery_id == delivery_id)
        else {
            return Ok(());
        };
        let mut next = state.clone();
        next.outbox.remove(position);
        self.state_file.commit(&next)?;
        *state = next;
        Ok(())
    }

    async fn next_inbox(&self) -> Option<DurableDelivery> {
        let state = self.state.lock().await;
        let mut leased = self.leased.lock();
        let now = Instant::now();
        leased.retain(|_, expires_at| *expires_at > now);
        // Do not lease later messages around an unacknowledged head entry.
        // This preserves ordered delivery during failure and retry, not merely
        // ordered insertion into the durable inbox.
        let delivery = state.inbox.first()?.clone();
        if leased.contains_key(&delivery.delivery_id) {
            return None;
        }
        leased.insert(
            delivery.delivery_id.clone(),
            now + self.config.application_lease_timeout,
        );
        Some(delivery)
    }

    async fn acknowledge_inbox(&self, delivery_id: &str) -> Result<(), PcaError> {
        let mut state = self.state.lock().await;
        let Some(position) = state
            .inbox
            .iter()
            .position(|entry| entry.delivery_id == delivery_id)
        else {
            return Err(PcaError::ChannelError {
                reason: format!("unknown durable inbox delivery ID: {delivery_id}"),
            });
        };
        let mut next = state.clone();
        next.inbox.remove(position);
        self.state_file.commit(&next)?;
        *state = next;
        self.leased.lock().remove(delivery_id);
        Ok(())
    }

    fn max_frame_bytes(&self) -> u64 {
        self.config
            .max_message_bytes
            // Ciphertext bytes are represented as JSON integers in the v1
            // frame. Four bytes per input byte is the worst case; retain one
            // additional multiple for envelope metadata.
            .saturating_mul(5)
            .saturating_add(FRAME_OVERHEAD_BYTES)
    }
}

impl Drop for TcpPcaTransport {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.shutdown_notify.notify_waiters();
        self.inbox_notify.notify_waiters();
        self.outbox_notify.notify_waiters();
        for task in self.tasks.get_mut().drain(..) {
            task.abort();
        }
    }
}

#[async_trait]
impl Transport for TcpPcaTransport {
    async fn receive(&self) -> Result<IncomingMessage, TransportError> {
        loop {
            let notified = self.inbox_notify.notified();
            if let Some(delivery) = self.next_inbox().await {
                let wire: PcaWireMessage =
                    serde_json::from_slice(&delivery.payload).map_err(|error| {
                        TransportError::Internal {
                            message: format!("persisted PCA wire message is invalid: {error}"),
                        }
                    })?;
                let conversation_id =
                    ConversationId::from_str(&wire.conversation_id).map_err(|error| {
                        TransportError::Internal {
                            message: format!("persisted conversation ID is invalid: {error}"),
                        }
                    })?;
                return Ok(IncomingMessage {
                    delivery_id: DeliveryId::new(delivery.delivery_id),
                    conversation_id,
                    sender: AuthenticatedSender {
                        user_id: UserId::new(wire.sender_address),
                        display_name: wire.sender_display_name,
                        trust_tier: SenderTrustTier::Authenticated,
                    },
                    body: decode_wire_body(wire.body_json),
                    received_at: now(),
                });
            }
            if self.shutdown.load(Ordering::SeqCst) {
                return Err(TransportError::ConnectionLost);
            }
            tokio::select! {
                () = notified => {},
                () = tokio::time::sleep(self.config.application_lease_timeout) => {},
            }
        }
    }

    async fn ack(&self, delivery_id: DeliveryId) -> Result<(), TransportError> {
        self.acknowledge_inbox(&delivery_id.0)
            .await
            .map_err(TransportError::from)
    }

    async fn send(&self, message: OutgoingMessage) -> Result<DeliveryReceipt, TransportError> {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(TransportError::ConnectionLost);
        }
        let body_json =
            serde_json::to_string(&message.body).map_err(|error| TransportError::Internal {
                message: format!("failed to serialize outgoing body: {error}"),
            })?;
        let wire = PcaWireMessage {
            conversation_id: message.conversation_id.to_string(),
            sender_address: self.config.local_ss58_address.clone(),
            sender_display_name: None,
            body_json,
        };
        let payload = serde_json::to_vec(&wire).map_err(|error| TransportError::Internal {
            message: format!("failed to serialize PCA wire message: {error}"),
        })?;
        if payload.len() as u64 > self.config.max_message_bytes {
            return Err(TransportError::DeliveryFailed {
                conversation_id: message.conversation_id.to_string(),
                message: format!(
                    "message size {} exceeds max {}",
                    payload.len(),
                    self.config.max_message_bytes
                ),
            });
        }

        let delivery_id = format!("pca-tcp-{}", Uuid::now_v7());
        let mut state = self.state.lock().await;
        if state.outbox.len() >= self.config.max_in_flight {
            return Err(TransportError::RateLimit {
                retry_after: Some(self.config.retry_interval),
            });
        }
        let mut next = state.clone();
        next.outbox.push(DurableDelivery {
            delivery_id: delivery_id.clone(),
            payload,
        });
        self.state_file
            .commit(&next)
            .map_err(TransportError::from)?;
        *state = next;
        drop(state);
        self.outbox_notify.notify_one();
        Ok(DeliveryReceipt {
            delivery_id: DeliveryId::new(delivery_id),
            delivered_at: now(),
        })
    }

    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            supports_streaming: false,
            supports_structured_cards: true,
            supports_file_transfer: false,
            max_message_bytes: self.config.max_message_bytes,
            supported_auth_methods: vec![
                "x25519-chacha20poly1305".into(),
                "configured-ss58-peer".into(),
            ],
        }
    }
}

fn validate_wire_payload(payload: &[u8], peer_address: &str) -> Result<(), PcaError> {
    let wire: PcaWireMessage =
        serde_json::from_slice(payload).map_err(|error| PcaError::ProtocolError {
            reason: format!("invalid PCA wire message: {error}"),
        })?;
    if wire.sender_address != peer_address {
        return Err(PcaError::PeerAuthenticationFailed {
            peer_address: wire.sender_address,
            reason: "message sender differs from handshake identity".into(),
        });
    }
    ConversationId::from_str(&wire.conversation_id).map_err(|error| PcaError::ProtocolError {
        reason: format!("invalid conversation ID: {error}"),
    })?;
    Ok(())
}

fn decode_wire_body(body_json: String) -> MessageBody {
    match serde_json::from_str::<OutgoingBody>(&body_json) {
        Ok(OutgoingBody::Text { content }) => MessageBody::Text { content },
        Ok(OutgoingBody::StructuredCard {
            card_type,
            payload_json,
        }) => MessageBody::StructuredCommand {
            command_type: card_type,
            payload_json,
        },
        Ok(OutgoingBody::ApprovalRequest { payload_json }) => MessageBody::StructuredCommand {
            command_type: "approval_request".into(),
            payload_json,
        },
        // Preserve compatibility with PCA payload producers that place plain
        // text rather than a tagged Polkagent body in `body_json`.
        Err(_) => MessageBody::Text { content: body_json },
    }
}

async fn write_frame(
    stream: &mut TcpStream,
    frame: &WireFrame,
    max_frame_bytes: u64,
) -> Result<(), PcaError> {
    let bytes = serde_json::to_vec(frame).map_err(|error| PcaError::ProtocolError {
        reason: format!("failed to encode frame: {error}"),
    })?;
    if bytes.len() as u64 > max_frame_bytes || bytes.len() > u32::MAX as usize {
        return Err(PcaError::ProtocolError {
            reason: format!(
                "frame size {} exceeds maximum {max_frame_bytes}",
                bytes.len()
            ),
        });
    }
    let length = u32::try_from(bytes.len()).map_err(|error| PcaError::ProtocolError {
        reason: format!("frame length cannot be represented as u32: {error}"),
    })?;
    stream.write_u32(length).await.map_err(PcaError::from)?;
    stream.write_all(&bytes).await.map_err(PcaError::from)?;
    stream.flush().await.map_err(PcaError::from)
}

async fn read_frame(stream: &mut TcpStream, max_frame_bytes: u64) -> Result<WireFrame, PcaError> {
    let length = stream.read_u32().await.map_err(PcaError::from)?;
    if u64::from(length) > max_frame_bytes {
        return Err(PcaError::ProtocolError {
            reason: format!("frame size {length} exceeds maximum {max_frame_bytes}"),
        });
    }
    let mut bytes = vec![0_u8; length as usize];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(PcaError::from)?;
    serde_json::from_slice(&bytes).map_err(|error| PcaError::ProtocolError {
        reason: format!("failed to decode frame: {error}"),
    })
}

async fn read_frame_until_shutdown(
    stream: &mut TcpStream,
    max_frame_bytes: u64,
    shutdown: &AtomicBool,
    shutdown_notify: &Notify,
) -> Result<WireFrame, PcaError> {
    if shutdown.load(Ordering::SeqCst) {
        return Err(PcaError::Shutdown);
    }
    tokio::select! {
        () = shutdown_notify.notified() => Err(PcaError::Shutdown),
        frame = read_frame(stream, max_frame_bytes) => frame,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_commit_round_trips() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("state.json");
        let (store, state) = StateFile::open(path.clone()).expect("open state");
        store.commit(&state).expect("commit state");
        let (_, loaded) = StateFile::open(path).expect("reload state");
        assert_eq!(loaded.version, STATE_VERSION);
        assert!(loaded.inbox.is_empty());
    }

    #[test]
    fn corrupt_state_is_rejected() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("state.json");
        fs::write(&path, b"not json").expect("write corrupt state");
        assert!(StateFile::open(path).is_err());
    }
}
