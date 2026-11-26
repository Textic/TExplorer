//! Network VFS backends (SSH/SFTP, FTP/FTPS) and encrypted Internet+LAN P2P transfer engine.

pub mod p2p;
pub mod remote;

pub use p2p::{
    ActiveP2pShare, IncomingP2pOffer, NearbyPeer, P2P_ALPN, P2pDirection, P2pEvent, P2pService,
    P2pTicket, decode_ticket, encode_ticket, sanitize_relative_p2p_path,
};
pub use remote::{
    RemoteClientPool, RemoteCredentials, detect_default_ssh_key, parse_ftp_list_entries,
};
