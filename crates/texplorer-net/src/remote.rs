//! Persistent remote filesystem client pool supporting SSH/SFTP (`russh` + `russh-sftp`)
//! and FTP / Explicit FTPS (`suppaftp` + Windows SChannel `native-tls`).

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use russh::client::{self, Handle};
use russh::keys::{PrivateKeyWithHashAlg, load_secret_key};
use russh_sftp::client::SftpSession;
use suppaftp::native_tls::TlsConnector;
use suppaftp::types::FileType as FtpTransferType;
use suppaftp::{FtpStream, NativeTlsConnector, NativeTlsFtpStream};
use texplorer_core::{
    EntryFlags, FileEntry, FileKind, Location, NetworkConnection, NetworkProtocol, SshAuthMode,
    classify_extension, join_remote_path, normalize_remote_path,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use uuid::Uuid;

/// Runtime credentials (loaded from Windows Credential Manager or entered in-memory).
#[derive(Debug, Clone, Default)]
pub struct RemoteCredentials {
    pub password: Option<String>,
}

struct SshClientHandler;

impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Accept host keys for interactive file-manager sessions (similar to initial trust-on-use).
        Ok(true)
    }
}

struct CachedSftpSession {
    _ssh_handle: Handle<SshClientHandler>,
    sftp: Arc<SftpSession>,
    endpoint_sig: String,
}

enum FtpClient {
    Plain(FtpStream),
    Secure(NativeTlsFtpStream),
}

impl FtpClient {
    fn cwd(&mut self, path: &str) -> Result<(), String> {
        match self {
            Self::Plain(s) => s.cwd(path),
            Self::Secure(s) => s.cwd(path),
        }
        .map_err(|e| format!("FTP CWD \"{path}\" failed: {e}"))
    }

    fn list(&mut self, path: Option<&str>) -> Result<Vec<String>, String> {
        match self {
            Self::Plain(s) => s.list(path),
            Self::Secure(s) => s.list(path),
        }
        .map_err(|e| format!("FTP LIST failed: {e}"))
    }

    fn retr_as_buffer(&mut self, path: &str) -> Result<Cursor<Vec<u8>>, String> {
        match self {
            Self::Plain(s) => s.retr_as_buffer(path),
            Self::Secure(s) => s.retr_as_buffer(path),
        }
        .map_err(|e| format!("FTP download \"{path}\" failed: {e}"))
    }

    fn retr_to_writer(&mut self, path: &str, dest: &Path) -> Result<u64, String> {
        let mut out = std::fs::File::create(dest)
            .map_err(|e| format!("Failed creating local file \"{}\": {e}", dest.display()))?;
        let copy_fn = |stream: &mut dyn Read| -> Result<u64, suppaftp::FtpError> {
            std::io::copy(stream, &mut out).map_err(suppaftp::FtpError::ConnectionError)
        };
        match self {
            Self::Plain(s) => s.retr(path, copy_fn),
            Self::Secure(s) => s.retr(path, copy_fn),
        }
        .map_err(|e| format!("FTP download \"{path}\" failed: {e}"))
    }

    fn put_file(&mut self, remote_path: &str, src: &Path) -> Result<u64, String> {
        let mut file = std::fs::File::open(src)
            .map_err(|e| format!("Failed opening local file \"{}\": {e}", src.display()))?;
        match self {
            Self::Plain(s) => s.put_file(remote_path, &mut file),
            Self::Secure(s) => s.put_file(remote_path, &mut file),
        }
        .map_err(|e| format!("FTP upload \"{remote_path}\" failed: {e}"))
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        match self {
            Self::Plain(s) => s.mkdir(path),
            Self::Secure(s) => s.mkdir(path),
        }
        .map_err(|e| format!("FTP mkdir \"{path}\" failed: {e}"))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        match self {
            Self::Plain(s) => s.rename(from, to),
            Self::Secure(s) => s.rename(from, to),
        }
        .map_err(|e| format!("FTP rename failed: {e}"))
    }

    fn rm(&mut self, path: &str) -> Result<(), String> {
        match self {
            Self::Plain(s) => s.rm(path),
            Self::Secure(s) => s.rm(path),
        }
        .map_err(|e| format!("FTP delete file \"{path}\" failed: {e}"))
    }

    fn rmdir(&mut self, path: &str) -> Result<(), String> {
        match self {
            Self::Plain(s) => s.rmdir(path),
            Self::Secure(s) => s.rmdir(path),
        }
        .map_err(|e| format!("FTP delete folder \"{path}\" failed: {e}"))
    }

    fn quit(&mut self) {
        match self {
            Self::Plain(s) => {
                let _ = s.quit();
            }
            Self::Secure(s) => {
                let _ = s.quit();
            }
        }
    }
}

/// Shared async connection pool managing active SFTP sessions and FTP/FTPS operations.
#[derive(Clone, Default)]
pub struct RemoteClientPool {
    sftp_sessions: Arc<Mutex<HashMap<Uuid, CachedSftpSession>>>,
    active_ids: Arc<std::sync::Mutex<HashMap<Uuid, bool>>>,
}

impl RemoteClientPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the IDs of remote connections that currently have an active session.
    pub fn connected_ids(&self) -> Vec<Uuid> {
        self.active_ids
            .lock()
            .map(|m| m.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn is_connected(&self, id: Uuid) -> bool {
        self.active_ids
            .lock()
            .map(|m| m.contains_key(&id))
            .unwrap_or(false)
    }

    fn mark_connected(&self, id: Uuid) {
        if let Ok(mut m) = self.active_ids.lock() {
            m.insert(id, true);
        }
    }

    fn mark_disconnected(&self, id: Uuid) {
        if let Ok(mut m) = self.active_ids.lock() {
            m.remove(&id);
        }
    }

    /// Closes and evicts any cached session for `id`.
    pub async fn disconnect(&self, id: Uuid) {
        let removed = {
            let mut guard = self.sftp_sessions.lock().await;
            guard.remove(&id)
        };
        if let Some(session) = removed {
            let _ = session.sftp.close().await;
        }
        self.mark_disconnected(id);
    }

    /// Tests connectivity and authentication against `conn`.
    pub async fn test_connection(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
    ) -> Result<String, String> {
        let start_path = conn.normalized_remote_path();
        let entries = self.list_dir(conn, creds, &start_path).await?;
        Ok(format!(
            "Connected to {} ({} items in {start_path})",
            conn.display_name(),
            entries.len()
        ))
    }

    /// Lists entries in `remote_path` on an SFTP, FTP, or FTPS server.
    pub async fn list_dir(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
        remote_path: &str,
    ) -> Result<Vec<FileEntry>, String> {
        let norm_path = normalize_remote_path(remote_path);
        match conn.protocol {
            NetworkProtocol::Sftp => self.sftp_list_dir(conn, creds, &norm_path).await,
            NetworkProtocol::Ftp | NetworkProtocol::Ftps => {
                let conn_owned = conn.clone();
                let creds_owned = creds.clone();
                let pool = self.clone();
                tokio::task::spawn_blocking(move || {
                    let entries = ftp_list_dir_blocking(&conn_owned, &creds_owned, &norm_path)?;
                    pool.mark_connected(conn_owned.id);
                    Ok(entries)
                })
                .await
                .map_err(|e| format!("FTP worker panicked: {e}"))?
            }
            NetworkProtocol::Smb => {
                Err("SMB shares are accessed directly as local UNC paths.".to_string())
            }
        }
    }

    /// Reads up to `max_bytes` from a remote file for inline QuickLook / Inspector previews.
    pub async fn read_file_bytes(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
        remote_path: &str,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, bool), String> {
        let norm_path = normalize_remote_path(remote_path);
        match conn.protocol {
            NetworkProtocol::Sftp => {
                let sftp = self.get_or_connect_sftp(conn, creds).await?;
                let mut file = sftp
                    .open(&norm_path)
                    .await
                    .map_err(|e| format!("SFTP open \"{norm_path}\" failed: {e}"))?;
                let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024));
                let mut chunk = [0u8; 16 * 1024];
                let mut truncated = false;
                loop {
                    let n = file
                        .read(&mut chunk)
                        .await
                        .map_err(|e| format!("SFTP read \"{norm_path}\" failed: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    let remaining = max_bytes.saturating_sub(buf.len());
                    if n > remaining {
                        buf.extend_from_slice(&chunk[..remaining]);
                        truncated = true;
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() >= max_bytes {
                        let mut probe = [0u8; 1];
                        if file.read(&mut probe).await.unwrap_or(0) > 0 {
                            truncated = true;
                        }
                        break;
                    }
                }
                let _ = file.close().await;
                Ok((buf, truncated))
            }
            NetworkProtocol::Ftp | NetworkProtocol::Ftps => {
                let conn_owned = conn.clone();
                let creds_owned = creds.clone();
                tokio::task::spawn_blocking(move || {
                    let mut client = connect_ftp_blocking(&conn_owned, &creds_owned)?;
                    let cursor = client.retr_as_buffer(&norm_path)?;
                    client.quit();
                    let mut data = cursor.into_inner();
                    let truncated = data.len() > max_bytes;
                    if truncated {
                        data.truncate(max_bytes);
                    }
                    Ok((data, truncated))
                })
                .await
                .map_err(|e| format!("FTP worker panicked: {e}"))?
            }
            NetworkProtocol::Smb => Err("Use local file reading for SMB shares.".to_string()),
        }
    }

    /// Downloads a remote file to `local_dest` on disk.
    pub async fn download_file(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
        remote_path: &str,
        local_dest: &Path,
    ) -> Result<u64, String> {
        let norm_path = normalize_remote_path(remote_path);
        if let Some(parent) = local_dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match conn.protocol {
            NetworkProtocol::Sftp => {
                let sftp = self.get_or_connect_sftp(conn, creds).await?;
                let mut remote = sftp
                    .open(&norm_path)
                    .await
                    .map_err(|e| format!("SFTP open \"{norm_path}\" failed: {e}"))?;
                let mut local = tokio::fs::File::create(local_dest)
                    .await
                    .map_err(|e| format!("Failed creating \"{}\": {e}", local_dest.display()))?;
                let mut buf = vec![0u8; 64 * 1024];
                let mut total = 0u64;
                loop {
                    let n = remote
                        .read(&mut buf)
                        .await
                        .map_err(|e| format!("SFTP download failed: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    local
                        .write_all(&buf[..n])
                        .await
                        .map_err(|e| format!("Failed writing local file: {e}"))?;
                    total = total.saturating_add(n as u64);
                }
                local
                    .flush()
                    .await
                    .map_err(|e| format!("Failed flushing local file: {e}"))?;
                let _ = remote.close().await;
                Ok(total)
            }
            NetworkProtocol::Ftp | NetworkProtocol::Ftps => {
                let conn_owned = conn.clone();
                let creds_owned = creds.clone();
                let dest_owned = local_dest.to_path_buf();
                tokio::task::spawn_blocking(move || {
                    let mut client = connect_ftp_blocking(&conn_owned, &creds_owned)?;
                    let bytes = client.retr_to_writer(&norm_path, &dest_owned)?;
                    client.quit();
                    Ok(bytes)
                })
                .await
                .map_err(|e| format!("FTP worker panicked: {e}"))?
            }
            NetworkProtocol::Smb => Err("Use local copy for SMB shares.".to_string()),
        }
    }

    /// Uploads a local file to `remote_dest` on the remote server.
    pub async fn upload_file(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
        local_src: &Path,
        remote_dest: &str,
    ) -> Result<u64, String> {
        let norm_dest = normalize_remote_path(remote_dest);
        match conn.protocol {
            NetworkProtocol::Sftp => {
                let sftp = self.get_or_connect_sftp(conn, creds).await?;
                let mut local = tokio::fs::File::open(local_src)
                    .await
                    .map_err(|e| format!("Failed opening \"{}\": {e}", local_src.display()))?;
                let mut remote = sftp
                    .create(&norm_dest)
                    .await
                    .map_err(|e| format!("SFTP create \"{norm_dest}\" failed: {e}"))?;
                let mut buf = vec![0u8; 64 * 1024];
                let mut total = 0u64;
                loop {
                    let n = local
                        .read(&mut buf)
                        .await
                        .map_err(|e| format!("Failed reading local file: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    remote
                        .write_all(&buf[..n])
                        .await
                        .map_err(|e| format!("SFTP upload failed: {e}"))?;
                    total = total.saturating_add(n as u64);
                }
                remote
                    .flush()
                    .await
                    .map_err(|e| format!("SFTP flush failed: {e}"))?;
                let _ = remote.close().await;
                Ok(total)
            }
            NetworkProtocol::Ftp | NetworkProtocol::Ftps => {
                let conn_owned = conn.clone();
                let creds_owned = creds.clone();
                let src_owned = local_src.to_path_buf();
                tokio::task::spawn_blocking(move || {
                    let mut client = connect_ftp_blocking(&conn_owned, &creds_owned)?;
                    let bytes = client.put_file(&norm_dest, &src_owned)?;
                    client.quit();
                    Ok(bytes)
                })
                .await
                .map_err(|e| format!("FTP worker panicked: {e}"))?
            }
            NetworkProtocol::Smb => Err("Use local copy for SMB shares.".to_string()),
        }
    }

    /// Creates a new directory on the remote server.
    pub async fn create_dir(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
        remote_dir: &str,
    ) -> Result<(), String> {
        let norm = normalize_remote_path(remote_dir);
        match conn.protocol {
            NetworkProtocol::Sftp => {
                let sftp = self.get_or_connect_sftp(conn, creds).await?;
                sftp.create_dir(&norm)
                    .await
                    .map_err(|e| format!("SFTP mkdir \"{norm}\" failed: {e}"))
            }
            NetworkProtocol::Ftp | NetworkProtocol::Ftps => {
                let conn_owned = conn.clone();
                let creds_owned = creds.clone();
                tokio::task::spawn_blocking(move || {
                    let mut client = connect_ftp_blocking(&conn_owned, &creds_owned)?;
                    let res = client.mkdir(&norm);
                    client.quit();
                    res
                })
                .await
                .map_err(|e| format!("FTP worker panicked: {e}"))?
            }
            NetworkProtocol::Smb => Err("Use local folder creation for SMB shares.".to_string()),
        }
    }

    /// Renames or moves a file/folder on the remote server.
    pub async fn rename(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
        old_path: &str,
        new_path: &str,
    ) -> Result<(), String> {
        let old_norm = normalize_remote_path(old_path);
        let new_norm = normalize_remote_path(new_path);
        match conn.protocol {
            NetworkProtocol::Sftp => {
                let sftp = self.get_or_connect_sftp(conn, creds).await?;
                sftp.rename(&old_norm, &new_norm)
                    .await
                    .map_err(|e| format!("SFTP rename failed: {e}"))
            }
            NetworkProtocol::Ftp | NetworkProtocol::Ftps => {
                let conn_owned = conn.clone();
                let creds_owned = creds.clone();
                tokio::task::spawn_blocking(move || {
                    let mut client = connect_ftp_blocking(&conn_owned, &creds_owned)?;
                    let res = client.rename(&old_norm, &new_norm);
                    client.quit();
                    res
                })
                .await
                .map_err(|e| format!("FTP worker panicked: {e}"))?
            }
            NetworkProtocol::Smb => Err("Use local rename for SMB shares.".to_string()),
        }
    }

    /// Deletes a remote file or empty remote directory.
    pub async fn delete_entry(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
        remote_path: &str,
        is_dir: bool,
    ) -> Result<(), String> {
        let norm = normalize_remote_path(remote_path);
        match conn.protocol {
            NetworkProtocol::Sftp => {
                let sftp = self.get_or_connect_sftp(conn, creds).await?;
                if is_dir {
                    sftp.remove_dir(&norm)
                        .await
                        .map_err(|e| format!("SFTP delete folder \"{norm}\" failed: {e}"))
                } else {
                    sftp.remove_file(&norm)
                        .await
                        .map_err(|e| format!("SFTP delete file \"{norm}\" failed: {e}"))
                }
            }
            NetworkProtocol::Ftp | NetworkProtocol::Ftps => {
                let conn_owned = conn.clone();
                let creds_owned = creds.clone();
                tokio::task::spawn_blocking(move || {
                    let mut client = connect_ftp_blocking(&conn_owned, &creds_owned)?;
                    let res = if is_dir {
                        client.rmdir(&norm)
                    } else {
                        client.rm(&norm)
                    };
                    client.quit();
                    res
                })
                .await
                .map_err(|e| format!("FTP worker panicked: {e}"))?
            }
            NetworkProtocol::Smb => Err("Use local delete for SMB shares.".to_string()),
        }
    }

    async fn sftp_list_dir(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
        norm_path: &str,
    ) -> Result<Vec<FileEntry>, String> {
        let sftp = self.get_or_connect_sftp(conn, creds).await?;
        let read_dir = match sftp.read_dir(norm_path).await {
            Ok(rd) => rd,
            Err(_) => {
                // Retry once with a fresh connection in case the server dropped an idle session.
                self.disconnect(conn.id).await;
                let fresh = self.get_or_connect_sftp(conn, creds).await?;
                fresh
                    .read_dir(norm_path)
                    .await
                    .map_err(|e| format!("SFTP read_dir \"{norm_path}\" failed: {e}"))?
            }
        };

        let label = conn.display_name();
        let mut entries = Vec::new();
        for dir_entry in read_dir {
            let name = dir_entry.file_name().to_string();
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            let meta = dir_entry.metadata();
            let ftype = dir_entry.file_type();
            let is_dir = ftype.is_dir();
            let is_symlink = ftype.is_symlink();
            let full_remote = join_remote_path(norm_path, &name);

            let ext_owned = if is_dir {
                String::new()
            } else {
                name.rsplit_once('.')
                    .map(|(_, e)| e.to_ascii_lowercase())
                    .unwrap_or_default()
            };
            let (kind, type_label, size) = if is_dir {
                (FileKind::Folder, "Remote Folder".to_string(), None)
            } else {
                let (k, lbl) = classify_extension(&ext_owned, false, is_symlink);
                (k, lbl, meta.size)
            };

            let mut flags = EntryFlags::empty();
            if is_dir {
                flags.insert(EntryFlags::IS_DIR);
            }
            if name.starts_with('.') {
                flags.insert(EntryFlags::IS_HIDDEN);
            }
            if is_symlink {
                flags.insert(EntryFlags::IS_SYMLINK);
            }

            let modified_secs = meta.mtime.map(i64::from).unwrap_or(0);
            entries.push(FileEntry {
                name,
                location: Location::remote(conn.id, label.clone(), conn.protocol, full_remote),
                kind,
                size,
                modified_secs,
                created_secs: modified_secs,
                flags,
                extension: ext_owned,
                type_label,
            });
        }
        Ok(entries)
    }

    async fn get_or_connect_sftp(
        &self,
        conn: &NetworkConnection,
        creds: &RemoteCredentials,
    ) -> Result<Arc<SftpSession>, String> {
        let sig = endpoint_signature(conn);
        {
            let guard = self.sftp_sessions.lock().await;
            if let Some(cached) = guard.get(&conn.id)
                && cached.endpoint_sig == sig
            {
                return Ok(Arc::clone(&cached.sftp));
            }
        }

        let session = establish_sftp_session(conn, creds, sig).await?;
        let sftp = Arc::clone(&session.sftp);
        {
            let mut guard = self.sftp_sessions.lock().await;
            guard.insert(conn.id, session);
        }
        self.mark_connected(conn.id);
        Ok(sftp)
    }
}

fn endpoint_signature(conn: &NetworkConnection) -> String {
    format!(
        "{}:{}:{}:{:?}:{:?}",
        conn.host.trim(),
        conn.port,
        conn.username.trim(),
        conn.auth_mode,
        conn.private_key_path
    )
}

async fn establish_sftp_session(
    conn: &NetworkConnection,
    creds: &RemoteCredentials,
    endpoint_sig: String,
) -> Result<CachedSftpSession, String> {
    let host = conn.host.trim();
    if host.is_empty() {
        return Err("Host cannot be empty.".to_string());
    }
    let port = if conn.port == 0 { 22 } else { conn.port };
    let username = conn.username.trim();
    if username.is_empty() {
        return Err("SSH username is required.".to_string());
    }

    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(300)),
        keepalive_interval: Some(Duration::from_secs(20)),
        ..Default::default()
    });

    let mut handle = tokio::time::timeout(
        Duration::from_secs(12),
        client::connect(config, (host, port), SshClientHandler),
    )
    .await
    .map_err(|_| format!("Timed out connecting to {host}:{port}."))?
    .map_err(|e| format!("SSH connection to {host}:{port} failed: {e}"))?;

    let authenticated = match conn.auth_mode {
        SshAuthMode::Password => {
            let password = creds.password.as_deref().unwrap_or("");
            handle
                .authenticate_password(username, password)
                .await
                .map_err(|e| format!("SSH password authentication error: {e}"))?
                .success()
        }
        SshAuthMode::PrivateKey => {
            let resolved_key = conn
                .private_key_path
                .clone()
                .filter(|p| !p.as_os_str().is_empty())
                .or_else(|| detect_default_ssh_key(Some(host)))
                .ok_or_else(|| {
                    "No SSH private key found in ~/.ssh. Select a private key file.".to_string()
                })?;
            let passphrase = creds
                .password
                .as_deref()
                .filter(|p| !p.is_empty());
            let key_pair = load_secret_key(&resolved_key, passphrase).map_err(|e| {
                format!(
                    "Failed to load SSH key \"{}\": {e}",
                    resolved_key.display()
                )
            })?;
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| format!("Failed negotiating SSH key algorithm: {e}"))?
                .flatten();
            let key_with_alg = PrivateKeyWithHashAlg::new(Arc::new(key_pair), hash_alg);
            handle
                .authenticate_publickey(username, key_with_alg)
                .await
                .map_err(|e| format!("SSH public-key authentication error: {e}"))?
                .success()
        }
    };

    if !authenticated {
        return Err(format!(
            "Authentication rejected by {host}:{port} for user \"{username}\"."
        ));
    }

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("Failed opening SSH session channel: {e}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("Server does not expose the SFTP subsystem: {e}"))?;
    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("Failed initializing SFTP protocol: {e}"))?;
    sftp.set_timeout(20);

    Ok(CachedSftpSession {
        _ssh_handle: handle,
        sftp: Arc::new(sftp),
        endpoint_sig,
    })
}

fn connect_ftp_blocking(
    conn: &NetworkConnection,
    creds: &RemoteCredentials,
) -> Result<FtpClient, String> {
    let host = conn.host.trim();
    if host.is_empty() {
        return Err("FTP host cannot be empty.".to_string());
    }
    let port = if conn.port == 0 { 21 } else { conn.port };
    let addr = format!("{host}:{port}");
    let user = if conn.username.trim().is_empty() {
        "anonymous"
    } else {
        conn.username.trim()
    };
    let pass = creds
        .password
        .as_deref()
        .unwrap_or(if user == "anonymous" {
            "texplorer@local"
        } else {
            ""
        });

    match conn.protocol {
        NetworkProtocol::Ftps => {
            let stream = NativeTlsFtpStream::connect(&addr)
                .map_err(|e| format!("FTPS connection to {addr} failed: {e}"))?;
            let connector = TlsConnector::new()
                .map_err(|e| format!("Failed initializing Windows TLS for FTPS: {e}"))?;
            let mut secure = stream
                .into_secure(NativeTlsConnector::from(connector), host)
                .map_err(|e| format!("FTPS TLS handshake with {host} failed: {e}"))?;
            secure
                .login(user, pass)
                .map_err(|e| format!("FTPS login failed for \"{user}\": {e}"))?;
            let _ = secure.transfer_type(FtpTransferType::Binary);
            Ok(FtpClient::Secure(secure))
        }
        _ => {
            let mut stream = FtpStream::connect(&addr)
                .map_err(|e| format!("FTP connection to {addr} failed: {e}"))?;
            stream
                .login(user, pass)
                .map_err(|e| format!("FTP login failed for \"{user}\": {e}"))?;
            let _ = stream.transfer_type(FtpTransferType::Binary);
            Ok(FtpClient::Plain(stream))
        }
    }
}

fn ftp_list_dir_blocking(
    conn: &NetworkConnection,
    creds: &RemoteCredentials,
    norm_path: &str,
) -> Result<Vec<FileEntry>, String> {
    let mut client = connect_ftp_blocking(conn, creds)?;
    client.cwd(norm_path)?;
    let raw_lines = client.list(None)?;
    client.quit();
    Ok(parse_ftp_list_entries(conn, norm_path, &raw_lines))
}

/// Converts raw FTP `LIST` output lines into typed `FileEntry` records.
pub fn parse_ftp_list_entries(
    conn: &NetworkConnection,
    parent_remote_path: &str,
    lines: &[String],
) -> Vec<FileEntry> {
    let label = conn.display_name();
    let mut entries = Vec::new();
    for raw in lines {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with("total ") {
            continue;
        }
        let parsed_opt = suppaftp::list::ListParser::parse_posix(trimmed)
            .or_else(|_| suppaftp::list::ListParser::parse_dos(trimmed))
            .ok()
            .or_else(|| {
                if trimmed.contains('=') && trimmed.contains(';') {
                    suppaftp::list::ListParser::parse_mlsd(trimmed).ok()
                } else {
                    None
                }
            });
        let (name, is_dir, is_symlink, raw_size, modified_secs) = if let Some(parsed) = parsed_opt {
            let m = parsed
                .modified()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            (
                parsed.name().to_string(),
                parsed.is_directory(),
                parsed.is_symlink(),
                parsed.size() as u64,
                m,
            )
        } else if let Some(fb) = parse_posix_list_fallback(trimmed) {
            fb
        } else {
            continue;
        };
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        let full_remote = join_remote_path(parent_remote_path, &name);

        let ext_owned = if is_dir {
            String::new()
        } else {
            name.rsplit_once('.')
                .map(|(_, e)| e.to_ascii_lowercase())
                .unwrap_or_default()
        };
        let (kind, type_label, size) = if is_dir {
            (FileKind::Folder, "Remote Folder".to_string(), None)
        } else {
            let (k, lbl) = classify_extension(&ext_owned, false, is_symlink);
            (k, lbl, Some(raw_size))
        };

        let mut flags = EntryFlags::empty();
        if is_dir {
            flags.insert(EntryFlags::IS_DIR);
        }
        if name.starts_with('.') {
            flags.insert(EntryFlags::IS_HIDDEN);
        }
        if is_symlink {
            flags.insert(EntryFlags::IS_SYMLINK);
        }

        entries.push(FileEntry {
            name,
            location: Location::remote(conn.id, label.clone(), conn.protocol, full_remote),
            kind,
            size,
            modified_secs,
            created_secs: modified_secs,
            flags,
            extension: ext_owned,
            type_label,
        });
    }
    entries
}

/// Resilient fallback parser for standard UNIX `ls -l` FTP lines when server date formatting
/// varies (e.g. zero-padded days like `Nov 05`).
fn parse_posix_list_fallback(line: &str) -> Option<(String, bool, bool, u64, i64)> {
    let first = line.as_bytes().first().copied()?;
    if !matches!(first, b'-' | b'd' | b'l') || line.len() < 12 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let perms = parts.next()?;
    if perms.len() < 10 {
        return None;
    }
    let _links = parts.next()?;
    let _owner = parts.next()?;
    let _group = parts.next()?;
    let size = parts.next()?.parse::<u64>().ok()?;
    let _month = parts.next()?;
    let _day = parts.next()?;
    let _time_or_year = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        return None;
    }
    let joined = rest.join(" ");
    let name = if first == b'l' {
        joined.split(" -> ").next().unwrap_or(&joined).to_string()
    } else {
        joined
    };
    Some((name, first == b'd', first == b'l', size, 0))
}

/// Detects the default OpenSSH private key that the system `ssh` client would use for `host`:
/// 1. Checks `~/.ssh/config` for a matching `Host` / `HostName` block with an `IdentityFile`.
/// 2. Falls back to standard OpenSSH default keys in `~/.ssh/` (`id_ed25519`, `id_ecdsa`,
///    `id_rsa`, `id_dsa`, `id_ed25519_sk`, `id_ecdsa_sk`).
pub fn detect_default_ssh_key(host: Option<&str>) -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    let ssh_dir = home.join(".ssh");
    let target_host = host.map(str::trim).filter(|h| !h.is_empty());

    let config_path = ssh_dir.join("config");
    if let Ok(raw_cfg) = std::fs::read_to_string(&config_path) {
        for candidate in parse_ssh_config_identity_files(&raw_cfg, target_host, &home) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    const DEFAULT_KEYS: [&str; 6] = [
        "id_ed25519",
        "id_ecdsa",
        "id_rsa",
        "id_dsa",
        "id_ed25519_sk",
        "id_ecdsa_sk",
    ];
    for name in DEFAULT_KEYS {
        let candidate = ssh_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn expand_ssh_tilde(raw: &str, home: &Path) -> PathBuf {
    let trimmed = raw.trim().trim_matches('"');
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix(r"~\"))
    {
        home.join(rest)
    } else if trimmed == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(trimmed)
    }
}

fn ssh_host_pattern_matches(pattern: &str, host: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let h = host.to_ascii_lowercase();
    if p == "*" {
        return true;
    }
    if !p.contains('*') && !p.contains('?') {
        return p == h;
    }
    let pb = p.as_bytes();
    let hb = h.as_bytes();
    let (mut pi, mut hi) = (0usize, 0usize);
    let (mut star_p, mut star_h) = (None::<usize>, 0usize);
    while hi < hb.len() {
        if pi < pb.len() && (pb[pi] == b'?' || pb[pi] == hb[hi]) {
            pi += 1;
            hi += 1;
        } else if pi < pb.len() && pb[pi] == b'*' {
            star_p = Some(pi);
            pi += 1;
            star_h = hi;
        } else if let Some(sp) = star_p {
            pi = sp + 1;
            star_h += 1;
            hi = star_h;
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == b'*' {
        pi += 1;
    }
    pi == pb.len()
}

fn parse_ssh_config_identity_files(
    raw: &str,
    target_host: Option<&str>,
    home: &Path,
) -> Vec<PathBuf> {
    struct Block {
        patterns: Vec<String>,
        hostname: Option<String>,
        identities: Vec<PathBuf>,
    }

    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Option<Block> = None;

    for line in raw.lines() {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, |c: char| c.is_whitespace() || c == '=');
        let key = parts.next().unwrap_or("").trim();
        let val = parts.next().unwrap_or("").trim().trim_start_matches('=').trim();
        if key.is_empty() || val.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("Host") {
            if let Some(prev) = current.take() {
                blocks.push(prev);
            }
            let patterns = val.split_whitespace().map(String::from).collect();
            current = Some(Block {
                patterns,
                hostname: None,
                identities: Vec::new(),
            });
        } else if let Some(block) = current.as_mut() {
            if key.eq_ignore_ascii_case("HostName") {
                block.hostname = Some(val.trim_matches('"').to_string());
            } else if key.eq_ignore_ascii_case("IdentityFile") {
                block.identities.push(expand_ssh_tilde(val, home));
            }
        }
    }
    if let Some(last) = current {
        blocks.push(last);
    }

    let mut specific = Vec::new();
    let mut wildcard = Vec::new();
    for block in blocks {
        if block.identities.is_empty() {
            continue;
        }
        let is_wild_only = block.patterns.iter().all(|p| p == "*");
        if let Some(host) = target_host {
            let mut matched = false;
            let mut negated = false;
            for pat in &block.patterns {
                if let Some(neg) = pat.strip_prefix('!') {
                    if ssh_host_pattern_matches(neg, host) {
                        negated = true;
                    }
                } else if ssh_host_pattern_matches(pat, host) {
                    matched = true;
                }
            }
            if !matched
                && let Some(hn) = block.hostname.as_deref()
                && hn.eq_ignore_ascii_case(host)
            {
                matched = true;
            }
            if matched && !negated {
                if is_wild_only {
                    wildcard.extend(block.identities);
                } else {
                    specific.extend(block.identities);
                }
            }
        } else if is_wild_only {
            wildcard.extend(block.identities);
        }
    }

    specific.extend(wildcard);
    specific
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_posix_ftp_list_output_into_file_entries() {
        let conn = NetworkConnection {
            name: "Demo FTP".to_string(),
            protocol: NetworkProtocol::Ftp,
            host: "ftp.example.com".to_string(),
            ..Default::default()
        };
        let lines = vec![
            "total 12".to_string(),
            "drwxr-xr-x 2 ftp ftp 4096 Nov 05 13:46 uploads".to_string(),
            "-rw-r--r-- 1 ftp ftp 2048 Nov 05 13:46 readme.txt".to_string(),
            "-rw-r--r-- 1 ftp ftp 128 Nov 05 13:46 .hidden".to_string(),
        ];
        let entries = parse_ftp_list_entries(&conn, "/pub", &lines);
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|e| e.name == "uploads" && e.is_dir()));
        assert!(entries.iter().any(|e| e.name == "readme.txt" && e.size == Some(2048)));
        assert!(
            entries
                .iter()
                .any(|e| e.name == ".hidden" && e.flags.contains(EntryFlags::IS_HIDDEN))
        );
    }

    #[test]
    fn parses_mlsd_ftp_list_output_into_file_entries() {
        let conn = NetworkConnection {
            name: "MLSD FTP".to_string(),
            protocol: NetworkProtocol::Ftps,
            host: "ftps.example.com".to_string(),
            ..Default::default()
        };
        let lines = vec![
            "type=dir;modify=20241105134600; releases".to_string(),
            "type=file;size=4096;modify=20241105134600; app.zip".to_string(),
        ];
        let entries = parse_ftp_list_entries(&conn, "/", &lines);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.name == "releases" && e.is_dir()));
        assert!(entries.iter().any(|e| e.name == "app.zip" && e.size == Some(4096)));
    }

    #[test]
    fn resolves_ssh_config_identity_files_for_host_and_wildcard() {
        let cfg = r#"
            # Prod VPS
            Host prod-vps
                HostName 10.0.0.5
                IdentityFile ~/.ssh/prod_ed25519

            Host *
                IdentityFile ~/.ssh/default_rsa
        "#;
        let home = PathBuf::from(r"C:\Users\demo");
        let by_alias = parse_ssh_config_identity_files(cfg, Some("prod-vps"), &home);
        assert_eq!(
            by_alias,
            vec![
                home.join(".ssh").join("prod_ed25519"),
                home.join(".ssh").join("default_rsa"),
            ]
        );
        let by_ip = parse_ssh_config_identity_files(cfg, Some("10.0.0.5"), &home);
        assert_eq!(by_ip[0], home.join(".ssh").join("prod_ed25519"));
        let other = parse_ssh_config_identity_files(cfg, Some("other.example.com"), &home);
        assert_eq!(other, vec![home.join(".ssh").join("default_rsa")]);
    }
}
