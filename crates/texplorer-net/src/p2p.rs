//! Encrypted Internet + LAN Peer-to-Peer (P2P) file & folder transfer engine built on
//! `iroh` (QUIC + TLS 1.3 + NAT/CGNAT hole-punching + encrypted relay fallback) plus
//! lightweight UDP LAN discovery so nearby PCs running TExplorer appear automatically.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

/// ALPN identifier negotiated over iroh QUIC connections.
pub const P2P_ALPN: &[u8] = b"texplorer/p2p/1";
const TICKET_PREFIX: &str = "texp1-";
const LAN_DISCOVERY_PORT: u16 = 45_821;
const LAN_MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 99);
const MAX_CONTROL_FRAME_BYTES: usize = 8 * 1024 * 1024;
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// A nearby PC running TExplorer discovered automatically on the local LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearbyPeer {
    pub id: String,
    pub device_name: String,
    pub addr: EndpointAddr,
}

/// Shareable ticket payload encoded into a `texp1-...` string for Internet & LAN transfers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pTicket {
    pub version: u8,
    pub addr: EndpointAddr,
    pub transfer_id: Uuid,
    pub secret: [u8; 16],
    pub sender_name: String,
    pub summary: String,
    pub item_count: usize,
    pub total_bytes: u64,
}

/// Metadata for an active outgoing P2P share waiting for a friend to fetch via ticket.
#[derive(Debug, Clone)]
pub struct ActiveP2pShare {
    pub transfer_id: Uuid,
    pub ticket_code: String,
    pub summary: String,
    pub item_count: usize,
    pub total_bytes: u64,
}

/// Incoming P2P transfer request from a nearby LAN peer awaiting receiver confirmation.
#[derive(Debug, Clone)]
pub struct IncomingP2pOffer {
    pub transfer_id: Uuid,
    pub sender_name: String,
    pub summary: String,
    pub item_count: usize,
    pub total_bytes: u64,
    pub sample_names: Vec<String>,
}

/// Direction of an active or completed P2P transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pDirection {
    Send,
    Receive,
}

/// Events emitted asynchronously from `P2pService` to the UI workspace.
#[derive(Debug, Clone)]
pub enum P2pEvent {
    EndpointReady {
        device_name: String,
        endpoint_id: String,
    },
    EndpointError(String),
    PeersUpdated(Vec<NearbyPeer>),
    IncomingOffer(IncomingP2pOffer),
    TransferProgress {
        transfer_id: Uuid,
        direction: P2pDirection,
        peer_name: String,
        summary: String,
        current_file: String,
        bytes_transferred: u64,
        total_bytes: u64,
    },
    TransferFinished {
        transfer_id: Uuid,
        direction: P2pDirection,
        peer_name: String,
        summary: String,
        dest_dir: Option<PathBuf>,
        result: Result<u64, String>,
    },
}

#[derive(Debug, Clone)]
struct PreparedFile {
    relative_path: String,
    source_path: PathBuf,
    size: u64,
}

#[derive(Debug, Clone)]
struct ShareOffer {
    secret: [u8; 16],
    summary: String,
    total_bytes: u64,
    files: Vec<PreparedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WireFileMeta {
    relative_path: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WireRequest {
    PullTicket {
        transfer_id: Uuid,
        secret: [u8; 16],
        receiver_name: String,
    },
    PushOffer {
        transfer_id: Uuid,
        sender_name: String,
        summary: String,
        total_bytes: u64,
        files: Vec<WireFileMeta>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WireResponse {
    Accepted,
    Declined { reason: String },
    Manifest {
        sender_name: String,
        summary: String,
        total_bytes: u64,
        files: Vec<WireFileMeta>,
    },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WireFileTrailer {
    sha256_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LanBeacon {
    version: u8,
    device_name: String,
    addr: EndpointAddr,
}

struct PeerRecord {
    peer: NearbyPeer,
    last_seen: Instant,
}

/// Manages TExplorer's encrypted P2P QUIC endpoint, active share tickets, incoming confirmations,
/// and automatic UDP LAN peer discovery.
#[derive(Clone)]
pub struct P2pService {
    device_name: String,
    endpoint: Arc<Mutex<Option<Endpoint>>>,
    shares: Arc<Mutex<HashMap<Uuid, ShareOffer>>>,
    pending_incoming: Arc<Mutex<HashMap<Uuid, oneshot::Sender<Option<PathBuf>>>>>,
    peers: Arc<StdMutex<HashMap<String, PeerRecord>>>,
    event_cb: Arc<dyn Fn(P2pEvent) + Send + Sync>,
}

impl P2pService {
    /// Creates a new `P2pService` and asynchronously boots the iroh QUIC endpoint + LAN discovery.
    pub fn start(
        device_name: impl Into<String>,
        on_event: impl Fn(P2pEvent) + Send + Sync + 'static,
    ) -> Self {
        let service = Self {
            device_name: device_name.into(),
            endpoint: Arc::new(Mutex::new(None)),
            shares: Arc::new(Mutex::new(HashMap::new())),
            pending_incoming: Arc::new(Mutex::new(HashMap::new())),
            peers: Arc::new(StdMutex::new(HashMap::new())),
            event_cb: Arc::new(on_event),
        };

        let boot_svc = service.clone();
        tokio::spawn(async move {
            match Endpoint::builder(presets::N0)
                .alpns(vec![P2P_ALPN.to_vec()])
                .bind()
                .await
            {
                Ok(ep) => {
                    let id_str = ep.id().to_string();
                    {
                        let mut guard = boot_svc.endpoint.lock().await;
                        *guard = Some(ep.clone());
                    }
                    (boot_svc.event_cb)(P2pEvent::EndpointReady {
                        device_name: boot_svc.device_name.clone(),
                        endpoint_id: id_str,
                    });

                    // Spawn incoming QUIC accept loop.
                    let accept_svc = boot_svc.clone();
                    let accept_ep = ep.clone();
                    tokio::spawn(async move {
                        accept_svc.run_accept_loop(accept_ep).await;
                    });

                    // Spawn lightweight LAN UDP peer discovery & beacon loop.
                    let lan_svc = boot_svc.clone();
                    let lan_ep = ep.clone();
                    tokio::spawn(async move {
                        lan_svc.run_lan_discovery(lan_ep).await;
                    });
                }
                Err(err) => {
                    (boot_svc.event_cb)(P2pEvent::EndpointError(format!(
                        "Failed to start P2P endpoint: {err}"
                    )));
                }
            }
        });

        service
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Returns currently discovered LAN peers.
    pub fn nearby_peers(&self) -> Vec<NearbyPeer> {
        self.peers
            .lock()
            .map(|map| {
                let mut list: Vec<NearbyPeer> = map.values().map(|r| r.peer.clone()).collect();
                list.sort_by(|a, b| a.device_name.cmp(&b.device_name));
                list
            })
            .unwrap_or_default()
    }

    /// Prepares `paths` (files and/or folders) for sharing via an encrypted Internet+LAN ticket.
    pub async fn create_share_offer(&self, paths: Vec<PathBuf>) -> Result<ActiveP2pShare, String> {
        let ep = self.wait_for_endpoint().await?;
        // Give iroh up to 2.5s to attach to its home relay so the ticket works across NAT/CGNAT too.
        let _ = tokio::time::timeout(Duration::from_millis(2500), ep.online()).await;

        let (files, summary, total_bytes) = tokio::task::spawn_blocking(move || {
            collect_transfer_manifest(&paths)
        })
        .await
        .map_err(|e| format!("Failed scanning files for P2P share: {e}"))??;

        let item_count = files.len();
        let transfer_id = Uuid::new_v4();
        let secret = *Uuid::new_v4().as_bytes();
        let addr = ep.addr();

        let ticket = P2pTicket {
            version: 1,
            addr,
            transfer_id,
            secret,
            sender_name: self.device_name.clone(),
            summary: summary.clone(),
            item_count,
            total_bytes,
        };
        let ticket_code = encode_ticket(&ticket)?;

        let offer = ShareOffer {
            secret,
            summary: summary.clone(),
            total_bytes,
            files,
        };
        {
            let mut guard = self.shares.lock().await;
            guard.insert(transfer_id, offer);
        }

        Ok(ActiveP2pShare {
            transfer_id,
            ticket_code,
            summary,
            item_count,
            total_bytes,
        })
    }

    /// Cancels an active share ticket so it can no longer be downloaded.
    pub async fn cancel_share_offer(&self, transfer_id: Uuid) {
        let mut guard = self.shares.lock().await;
        guard.remove(&transfer_id);
    }

    /// Connects to a sender using a pasted `texp1-...` ticket and downloads all files into `dest_dir`.
    pub async fn receive_from_ticket(
        &self,
        ticket_code: &str,
        dest_dir: PathBuf,
    ) -> Result<Uuid, String> {
        let ticket = decode_ticket(ticket_code)?;
        let ep = self.wait_for_endpoint().await?;
        let transfer_id = ticket.transfer_id;
        let svc = self.clone();

        tokio::spawn(async move {
            let outcome = svc.pull_ticket_transfer(ep, ticket.clone(), dest_dir.clone()).await;
            (svc.event_cb)(P2pEvent::TransferFinished {
                transfer_id,
                direction: P2pDirection::Receive,
                peer_name: ticket.sender_name,
                summary: ticket.summary,
                dest_dir: Some(dest_dir),
                result: outcome,
            });
        });

        Ok(transfer_id)
    }

    /// Pushes `paths` directly to a discovered nearby LAN peer (the receiver must confirm).
    pub async fn send_to_peer(
        &self,
        peer: NearbyPeer,
        paths: Vec<PathBuf>,
    ) -> Result<Uuid, String> {
        let ep = self.wait_for_endpoint().await?;
        let (files, summary, total_bytes) = tokio::task::spawn_blocking(move || {
            collect_transfer_manifest(&paths)
        })
        .await
        .map_err(|e| format!("Failed scanning files for P2P send: {e}"))??;

        let transfer_id = Uuid::new_v4();
        let svc = self.clone();
        let peer_name = peer.device_name.clone();
        let summary_done = summary.clone();

        tokio::spawn(async move {
            let outcome = svc
                .push_to_peer_transfer(ep, peer, transfer_id, summary.clone(), total_bytes, files)
                .await;
            (svc.event_cb)(P2pEvent::TransferFinished {
                transfer_id,
                direction: P2pDirection::Send,
                peer_name,
                summary: summary_done,
                dest_dir: None,
                result: outcome,
            });
        });

        Ok(transfer_id)
    }

    /// Accepts or declines a pending incoming LAN P2P transfer offer.
    pub async fn respond_to_incoming(&self, transfer_id: Uuid, dest_dir: Option<PathBuf>) {
        let tx = {
            let mut guard = self.pending_incoming.lock().await;
            guard.remove(&transfer_id)
        };
        if let Some(tx) = tx {
            let _ = tx.send(dest_dir);
        }
    }

    async fn wait_for_endpoint(&self) -> Result<Endpoint, String> {
        for _ in 0..40 {
            if let Some(ep) = self.endpoint.lock().await.clone() {
                return Ok(ep);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("P2P network endpoint is still initializing — try again in a moment.".to_string())
    }

    async fn pull_ticket_transfer(
        &self,
        ep: Endpoint,
        ticket: P2pTicket,
        dest_dir: PathBuf,
    ) -> Result<u64, String> {
        let conn = tokio::time::timeout(
            Duration::from_secs(20),
            ep.connect(ticket.addr.clone(), P2P_ALPN),
        )
        .await
        .map_err(|_| "Timed out connecting to sender peer.".to_string())?
        .map_err(|e| format!("Could not connect to sender peer: {e}"))?;

        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| format!("Failed opening P2P stream: {e}"))?;

        write_frame(
            &mut send,
            &WireRequest::PullTicket {
                transfer_id: ticket.transfer_id,
                secret: ticket.secret,
                receiver_name: self.device_name.clone(),
            },
        )
        .await?;
        let _ = send.finish();

        let resp: WireResponse = read_frame(&mut recv).await?;
        let (sender_name, summary, total_bytes, files) = match resp {
            WireResponse::Manifest {
                sender_name,
                summary,
                total_bytes,
                files,
            } => (sender_name, summary, total_bytes, files),
            WireResponse::Declined { reason } => return Err(reason),
            WireResponse::Error { message } => return Err(message),
            WireResponse::Accepted => {
                return Err("Unexpected response from sender peer.".to_string());
            }
        };

        let received = self
            .receive_file_stream(
                &mut recv,
                ticket.transfer_id,
                &sender_name,
                &summary,
                total_bytes,
                &files,
                &dest_dir,
            )
            .await?;

        conn.close(0u32.into(), b"done");
        Ok(received)
    }

    async fn push_to_peer_transfer(
        &self,
        ep: Endpoint,
        peer: NearbyPeer,
        transfer_id: Uuid,
        summary: String,
        total_bytes: u64,
        files: Vec<PreparedFile>,
    ) -> Result<u64, String> {
        let conn = tokio::time::timeout(
            Duration::from_secs(15),
            ep.connect(peer.addr.clone(), P2P_ALPN),
        )
        .await
        .map_err(|_| format!("Timed out connecting to {}.", peer.device_name))?
        .map_err(|e| format!("Could not connect to {}: {e}", peer.device_name))?;

        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| format!("Failed opening P2P stream: {e}"))?;

        let wire_files: Vec<WireFileMeta> = files
            .iter()
            .map(|f| WireFileMeta {
                relative_path: f.relative_path.clone(),
                size: f.size,
            })
            .collect();

        write_frame(
            &mut send,
            &WireRequest::PushOffer {
                transfer_id,
                sender_name: self.device_name.clone(),
                summary: summary.clone(),
                total_bytes,
                files: wire_files,
            },
        )
        .await?;

        let decision: WireResponse = tokio::time::timeout(
            Duration::from_secs(120),
            read_frame(&mut recv),
        )
        .await
        .map_err(|_| format!("{} did not respond in time.", peer.device_name))??;

        match decision {
            WireResponse::Accepted => {}
            WireResponse::Declined { reason } => return Err(reason),
            WireResponse::Error { message } => return Err(message),
            WireResponse::Manifest { .. } => {
                return Err("Unexpected response from receiver.".to_string());
            }
        }

        let sent = self
            .send_file_stream(
                &mut send,
                transfer_id,
                &peer.device_name,
                &summary,
                total_bytes,
                &files,
            )
            .await?;

        send.finish()
            .map_err(|e| format!("Failed finishing P2P send stream: {e}"))?;
        let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        Ok(sent)
    }

    async fn run_accept_loop(&self, ep: Endpoint) {
        while let Some(incoming) = ep.accept().await {
            let svc = self.clone();
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else {
                    return;
                };
                let Ok((mut send, mut recv)) = conn.accept_bi().await else {
                    return;
                };
                let Ok(req) = read_frame::<WireRequest, _>(&mut recv).await else {
                    return;
                };

                match req {
                    WireRequest::PullTicket {
                        transfer_id,
                        secret,
                        receiver_name,
                    } => {
                        let offer = {
                            let guard = svc.shares.lock().await;
                            guard.get(&transfer_id).cloned()
                        };
                        let Some(offer) = offer else {
                            let _ = write_frame(
                                &mut send,
                                &WireResponse::Error {
                                    message: "This P2P share code has expired or was cancelled."
                                        .to_string(),
                                },
                            )
                            .await;
                            let _ = send.finish();
                            return;
                        };
                        if !constant_time_eq_16(&offer.secret, &secret) {
                            let _ = write_frame(
                                &mut send,
                                &WireResponse::Declined {
                                    reason: "Invalid P2P security code.".to_string(),
                                },
                            )
                            .await;
                            let _ = send.finish();
                            return;
                        }

                        let wire_files: Vec<WireFileMeta> = offer
                            .files
                            .iter()
                            .map(|f| WireFileMeta {
                                relative_path: f.relative_path.clone(),
                                size: f.size,
                            })
                            .collect();
                        if write_frame(
                            &mut send,
                            &WireResponse::Manifest {
                                sender_name: svc.device_name.clone(),
                                summary: offer.summary.clone(),
                                total_bytes: offer.total_bytes,
                                files: wire_files,
                            },
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }

                        let outcome = svc
                            .send_file_stream(
                                &mut send,
                                transfer_id,
                                &receiver_name,
                                &offer.summary,
                                offer.total_bytes,
                                &offer.files,
                            )
                            .await;
                        let _ = send.finish();
                        let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;

                        (svc.event_cb)(P2pEvent::TransferFinished {
                            transfer_id,
                            direction: P2pDirection::Send,
                            peer_name: receiver_name,
                            summary: offer.summary,
                            dest_dir: None,
                            result: outcome,
                        });
                    }
                    WireRequest::PushOffer {
                        transfer_id,
                        sender_name,
                        summary,
                        total_bytes,
                        files,
                    } => {
                        let (tx, rx) = oneshot::channel::<Option<PathBuf>>();
                        {
                            let mut guard = svc.pending_incoming.lock().await;
                            guard.insert(transfer_id, tx);
                        }
                        let sample_names: Vec<String> = files
                            .iter()
                            .take(5)
                            .map(|f| f.relative_path.clone())
                            .collect();
                        (svc.event_cb)(P2pEvent::IncomingOffer(IncomingP2pOffer {
                            transfer_id,
                            sender_name: sender_name.clone(),
                            summary: summary.clone(),
                            item_count: files.len(),
                            total_bytes,
                            sample_names,
                        }));

                        let dest_opt = tokio::time::timeout(Duration::from_secs(120), rx)
                            .await
                            .ok()
                            .and_then(|r| r.ok())
                            .flatten();

                        // Clean up entry if timed out.
                        {
                            let mut guard = svc.pending_incoming.lock().await;
                            guard.remove(&transfer_id);
                        }

                        let Some(dest_dir) = dest_opt else {
                            let _ = write_frame(
                                &mut send,
                                &WireResponse::Declined {
                                    reason: "Transfer declined by receiver.".to_string(),
                                },
                            )
                            .await;
                            let _ = send.finish();
                            return;
                        };

                        if write_frame(&mut send, &WireResponse::Accepted)
                            .await
                            .is_err()
                        {
                            return;
                        }
                        let _ = send.finish();

                        let outcome = svc
                            .receive_file_stream(
                                &mut recv,
                                transfer_id,
                                &sender_name,
                                &summary,
                                total_bytes,
                                &files,
                                &dest_dir,
                            )
                            .await;
                        conn.close(0u32.into(), b"done");

                        (svc.event_cb)(P2pEvent::TransferFinished {
                            transfer_id,
                            direction: P2pDirection::Receive,
                            peer_name: sender_name,
                            summary,
                            dest_dir: Some(dest_dir),
                            result: outcome,
                        });
                    }
                }
            });
        }
    }

    async fn send_file_stream<W: AsyncWriteExt + Unpin>(
        &self,
        send: &mut W,
        transfer_id: Uuid,
        peer_name: &str,
        summary: &str,
        total_bytes: u64,
        files: &[PreparedFile],
    ) -> Result<u64, String> {
        let mut sent_total = 0u64;
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let mut buf = vec![0u8; STREAM_CHUNK_BYTES];

        for file in files {
            let mut fh = tokio::fs::File::open(&file.source_path).await.map_err(|e| {
                format!(
                    "Failed opening source file \"{}\": {e}",
                    file.source_path.display()
                )
            })?;
            let mut hasher = Sha256::new();
            let mut remaining = file.size;

            while remaining > 0 {
                let to_read = (remaining as usize).min(buf.len());
                let n = fh
                    .read(&mut buf[..to_read])
                    .await
                    .map_err(|e| format!("Failed reading \"{}\": {e}", file.relative_path))?;
                if n == 0 {
                    return Err(format!(
                        "Source file \"{}\" shrank while transferring.",
                        file.relative_path
                    ));
                }
                send.write_all(&buf[..n])
                    .await
                    .map_err(|e| format!("P2P send interrupted: {e}"))?;
                hasher.update(&buf[..n]);
                remaining -= n as u64;
                sent_total = sent_total.saturating_add(n as u64);

                if last_emit.elapsed() >= Duration::from_millis(80) {
                    (self.event_cb)(P2pEvent::TransferProgress {
                        transfer_id,
                        direction: P2pDirection::Send,
                        peer_name: peer_name.to_string(),
                        summary: summary.to_string(),
                        current_file: file.relative_path.clone(),
                        bytes_transferred: sent_total,
                        total_bytes,
                    });
                    last_emit = Instant::now();
                }
            }

            let trailer = WireFileTrailer {
                sha256_hex: hex_lower(&hasher.finalize()),
            };
            write_frame(send, &trailer).await?;
        }

        (self.event_cb)(P2pEvent::TransferProgress {
            transfer_id,
            direction: P2pDirection::Send,
            peer_name: peer_name.to_string(),
            summary: summary.to_string(),
            current_file: String::new(),
            bytes_transferred: sent_total,
            total_bytes,
        });
        Ok(sent_total)
    }

    #[allow(clippy::too_many_arguments)]
    async fn receive_file_stream<R: AsyncReadExt + Unpin>(
        &self,
        recv: &mut R,
        transfer_id: Uuid,
        peer_name: &str,
        summary: &str,
        total_bytes: u64,
        files: &[WireFileMeta],
        dest_dir: &Path,
    ) -> Result<u64, String> {
        tokio::fs::create_dir_all(dest_dir)
            .await
            .map_err(|e| format!("Failed creating destination \"{}\": {e}", dest_dir.display()))?;

        let mut received_total = 0u64;
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let mut buf = vec![0u8; STREAM_CHUNK_BYTES];

        for file in files {
            let safe_rel = sanitize_relative_p2p_path(&file.relative_path)?;
            let target = unique_dest_path(&dest_dir.join(&safe_rel));
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| format!("Failed creating folder \"{}\": {e}", parent.display()))?;
            }

            let mut out = tokio::fs::File::create(&target)
                .await
                .map_err(|e| format!("Failed creating \"{}\": {e}", target.display()))?;
            let mut hasher = Sha256::new();
            let mut remaining = file.size;

            while remaining > 0 {
                let to_read = (remaining as usize).min(buf.len());
                let n = recv
                    .read(&mut buf[..to_read])
                    .await
                    .map_err(|e| format!("P2P receive interrupted: {e}"))?;
                if n == 0 {
                    let _ = tokio::fs::remove_file(&target).await;
                    return Err(format!(
                        "Connection closed before \"{}\" finished.",
                        file.relative_path
                    ));
                }
                out.write_all(&buf[..n])
                    .await
                    .map_err(|e| format!("Failed writing \"{}\": {e}", target.display()))?;
                hasher.update(&buf[..n]);
                remaining -= n as u64;
                received_total = received_total.saturating_add(n as u64);

                if last_emit.elapsed() >= Duration::from_millis(80) {
                    (self.event_cb)(P2pEvent::TransferProgress {
                        transfer_id,
                        direction: P2pDirection::Receive,
                        peer_name: peer_name.to_string(),
                        summary: summary.to_string(),
                        current_file: file.relative_path.clone(),
                        bytes_transferred: received_total,
                        total_bytes,
                    });
                    last_emit = Instant::now();
                }
            }

            out.flush()
                .await
                .map_err(|e| format!("Failed flushing \"{}\": {e}", target.display()))?;
            drop(out);

            let trailer: WireFileTrailer = read_frame(recv).await?;
            let actual_hex = hex_lower(&hasher.finalize());
            if actual_hex != trailer.sha256_hex {
                let _ = tokio::fs::remove_file(&target).await;
                return Err(format!(
                    "SHA-256 verification failed for \"{}\".",
                    file.relative_path
                ));
            }
        }

        (self.event_cb)(P2pEvent::TransferProgress {
            transfer_id,
            direction: P2pDirection::Receive,
            peer_name: peer_name.to_string(),
            summary: summary.to_string(),
            current_file: String::new(),
            bytes_transferred: received_total,
            total_bytes,
        });
        Ok(received_total)
    }

    async fn run_lan_discovery(&self, ep: Endpoint) {
        let own_id = ep.id().to_string();

        // Bind UDP listener on LAN multicast/broadcast port (gracefully degrade to send-only if port is busy).
        let recv_sock = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            LAN_DISCOVERY_PORT,
        )))
        .await
        .ok();
        if let Some(ref s) = recv_sock {
            let _ = s.join_multicast_v4(LAN_MULTICAST_ADDR, Ipv4Addr::UNSPECIFIED);
            let _ = s.set_broadcast(true);
        }

        let send_sock = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            0,
        )))
        .await
        .ok();
        if let Some(ref s) = send_sock {
            let _ = s.set_broadcast(true);
        }

        // Spawn receiver task if bound.
        if let Some(rsock) = recv_sock {
            let svc = self.clone();
            let own_id_clone = own_id.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                loop {
                    let Ok((n, _src)) = rsock.recv_from(&mut buf).await else {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        continue;
                    };
                    let Ok(beacon) = serde_json::from_slice::<LanBeacon>(&buf[..n]) else {
                        continue;
                    };
                    if beacon.version != 1 {
                        continue;
                    }
                    let peer_id = beacon.addr.id.to_string();
                    if peer_id == own_id_clone {
                        continue;
                    }
                    let peer = NearbyPeer {
                        id: peer_id.clone(),
                        device_name: beacon.device_name,
                        addr: beacon.addr,
                    };
                    let changed = if let Ok(mut map) = svc.peers.lock() {
                        let is_new_or_changed = map
                            .get(&peer_id)
                            .is_none_or(|existing| existing.peer != peer);
                        map.insert(
                            peer_id,
                            PeerRecord {
                                peer,
                                last_seen: Instant::now(),
                            },
                        );
                        is_new_or_changed
                    } else {
                        false
                    };
                    if changed {
                        (svc.event_cb)(P2pEvent::PeersUpdated(svc.nearby_peers()));
                    }
                }
            });
        }

        // Periodic beacon sender + stale peer pruner.
        let mcast_target = SocketAddr::V4(SocketAddrV4::new(LAN_MULTICAST_ADDR, LAN_DISCOVERY_PORT));
        let bcast_target =
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, LAN_DISCOVERY_PORT));
        let mut interval = tokio::time::interval(Duration::from_secs(3));

        loop {
            interval.tick().await;
            if let Some(ref ssock) = send_sock {
                let beacon = LanBeacon {
                    version: 1,
                    device_name: self.device_name.clone(),
                    addr: ep.addr(),
                };
                if let Ok(payload) = serde_json::to_vec(&beacon) {
                    let _ = ssock.send_to(&payload, mcast_target).await;
                    let _ = ssock.send_to(&payload, bcast_target).await;
                }
            }

            let pruned = if let Ok(mut map) = self.peers.lock() {
                let before = map.len();
                map.retain(|_, rec| rec.last_seen.elapsed() < Duration::from_secs(12));
                map.len() != before
            } else {
                false
            };
            if pruned {
                (self.event_cb)(P2pEvent::PeersUpdated(self.nearby_peers()));
            }
        }
    }
}

/// Encodes a `P2pTicket` into a compact copyable string prefixed with `texp1-`.
pub fn encode_ticket(ticket: &P2pTicket) -> Result<String, String> {
    let json =
        serde_json::to_vec(ticket).map_err(|e| format!("Failed serializing P2P ticket: {e}"))?;
    Ok(format!("{TICKET_PREFIX}{}", URL_SAFE_NO_PAD.encode(json)))
}

/// Decodes and validates a pasted `texp1-...` ticket string.
pub fn decode_ticket(raw: &str) -> Result<P2pTicket, String> {
    let trimmed = raw.trim();
    let encoded = trimmed
        .strip_prefix(TICKET_PREFIX)
        .ok_or_else(|| "Invalid P2P code (expected a code starting with \"texp1-\").".to_string())?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "Corrupted P2P code — make sure you copied the full code.".to_string())?;
    let ticket: P2pTicket = serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid P2P code format.".to_string())?;
    if ticket.version != 1 {
        return Err(format!("Unsupported P2P code version ({}).", ticket.version));
    }
    Ok(ticket)
}

/// Strictly validates a relative path received over P2P so a remote peer can NEVER write
/// outside the receiver's chosen destination directory.
pub fn sanitize_relative_p2p_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Empty path in P2P transfer.".to_string());
    }
    let normalized = trimmed.replace('\\', "/");
    let mut out = PathBuf::new();
    for seg in normalized.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." || seg.contains(':') || seg.chars().any(|c| c.is_control()) {
            return Err(format!("Rejected unsafe P2P path \"{raw}\"."));
        }
        const FORBIDDEN: [char; 7] = ['<', '>', '"', '|', '?', '*', '\\'];
        if seg.chars().any(|c| FORBIDDEN.contains(&c)) {
            return Err(format!("Rejected invalid Windows filename \"{seg}\"."));
        }
        let candidate = Path::new(seg);
        if candidate.components().count() != 1
            || !matches!(candidate.components().next(), Some(Component::Normal(_)))
        {
            return Err(format!("Rejected unsafe P2P path segment \"{seg}\"."));
        }
        out.push(seg);
    }
    if out.as_os_str().is_empty() {
        return Err(format!("Invalid empty P2P path \"{raw}\"."));
    }
    Ok(out)
}

fn collect_transfer_manifest(paths: &[PathBuf]) -> Result<(Vec<PreparedFile>, String, u64), String> {
    if paths.is_empty() {
        return Err("Select at least one file or folder to share via P2P.".to_string());
    }
    let mut files = Vec::new();
    let mut total_bytes = 0u64;

    for path in paths {
        let base_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| format!("Cannot share root path \"{}\".", path.display()))?;
        if path.is_file() {
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            total_bytes = total_bytes.saturating_add(size);
            files.push(PreparedFile {
                relative_path: base_name,
                source_path: path.clone(),
                size,
            });
        } else if path.is_dir() {
            collect_dir_recursive(path, &base_name, &mut files, &mut total_bytes)?;
        }
    }

    if files.is_empty() {
        return Err("No readable files found in the selected items.".to_string());
    }

    let summary = if paths.len() == 1 {
        let first_name = paths[0]
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Transfer".to_string());
        if files.len() == 1 {
            first_name
        } else {
            format!("{first_name} ({} files)", files.len())
        }
    } else {
        format!("{} items ({} files)", paths.len(), files.len())
    };

    Ok((files, summary, total_bytes))
}

fn collect_dir_recursive(
    dir: &Path,
    prefix: &str,
    out: &mut Vec<PreparedFile>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    let rd = std::fs::read_dir(dir)
        .map_err(|e| format!("Failed reading folder \"{}\": {e}", dir.display()))?;
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{prefix}/{name}");
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            collect_dir_recursive(&path, &rel, out, total_bytes)?;
        } else if meta.is_file() {
            let size = meta.len();
            *total_bytes = total_bytes.saturating_add(size);
            out.push(PreparedFile {
                relative_path: rel,
                source_path: path,
                size,
            });
        }
    }
    Ok(())
}

fn unique_dest_path(target: &Path) -> PathBuf {
    if !target.exists() {
        return target.to_path_buf();
    }
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let ext = target
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..10_000 {
        let candidate = parent.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    target.to_path_buf()
}

async fn write_frame<T: Serialize, W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &T,
) -> Result<(), String> {
    let payload =
        serde_json::to_vec(msg).map_err(|e| format!("P2P frame serialization error: {e}"))?;
    let len = payload.len() as u32;
    writer
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| format!("P2P write frame length failed: {e}"))?;
    writer
        .write_all(&payload)
        .await
        .map_err(|e| format!("P2P write frame body failed: {e}"))?;
    Ok(())
}

async fn read_frame<T: for<'de> Deserialize<'de>, R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<T, String> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| format!("P2P stream closed while reading header: {e}"))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_CONTROL_FRAME_BYTES {
        return Err("Received invalid P2P control frame size.".to_string());
    }
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .await
        .map_err(|e| format!("P2P stream closed while reading payload: {e}"))?;
    serde_json::from_slice(&buf).map_err(|e| format!("Invalid P2P control frame: {e}"))
}

fn constant_time_eq_16(a: &[u8; 16], b: &[u8; 16]) -> bool {
    let mut diff = 0u8;
    for i in 0..16 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_and_accepts_safe_relative_paths() {
        assert!(sanitize_relative_p2p_path("../secret.txt").is_err());
        assert!(sanitize_relative_p2p_path("folder/../../evil.exe").is_err());
        assert!(sanitize_relative_p2p_path(r"C:\Windows\system32\cmd.exe").is_err());
        assert!(sanitize_relative_p2p_path("").is_err());

        assert_eq!(
            sanitize_relative_p2p_path("Photos/2026/beach.png").unwrap(),
            PathBuf::from("Photos").join("2026").join("beach.png")
        );
    }

    #[test]
    fn round_trips_p2p_ticket_and_manifest() {
        let key = iroh::SecretKey::from_bytes(&[7u8; 32]);
        let addr = EndpointAddr::new(key.public());
        let ticket = P2pTicket {
            version: 1,
            addr,
            transfer_id: Uuid::new_v4(),
            secret: [42u8; 16],
            sender_name: "PC-Daniel".to_string(),
            summary: "demo.zip".to_string(),
            item_count: 1,
            total_bytes: 1024,
        };
        let encoded = encode_ticket(&ticket).unwrap();
        assert!(encoded.starts_with(TICKET_PREFIX));
        let decoded = decode_ticket(&encoded).unwrap();
        assert_eq!(decoded.transfer_id, ticket.transfer_id);
        assert_eq!(decoded.sender_name, "PC-Daniel");
        assert_eq!(decoded.total_bytes, 1024);
    }
}
