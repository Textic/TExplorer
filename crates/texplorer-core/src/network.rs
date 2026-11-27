use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Supported remote network protocols in the sidebar Network section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProtocol {
    /// Windows SMB / CIFS shared folder (`\\server\share`).
    #[default]
    Smb,
    /// SSH File Transfer Protocol (`sftp://user@host:22/path`, standard over SSH/SCP servers).
    Sftp,
    /// Plain File Transfer Protocol (`ftp://host:21/path`).
    Ftp,
    /// Explicit TLS-secured File Transfer Protocol (`ftps://host:21/path`).
    Ftps,
}

impl NetworkProtocol {
    pub const ALL: [Self; 4] = [Self::Smb, Self::Sftp, Self::Ftp, Self::Ftps];

    pub fn label(self) -> &'static str {
        match self {
            Self::Smb => "Shared Folder (SMB)",
            Self::Sftp => "SSH / SFTP",
            Self::Ftp => "FTP",
            Self::Ftps => "FTPS (TLS)",
        }
    }

    pub fn short_badge(self) -> &'static str {
        match self {
            Self::Smb => "SMB",
            Self::Sftp => "SFTP",
            Self::Ftp => "FTP",
            Self::Ftps => "FTPS",
        }
    }

    pub fn scheme(self) -> &'static str {
        match self {
            Self::Smb => "smb",
            Self::Sftp => "sftp",
            Self::Ftp => "ftp",
            Self::Ftps => "ftps",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Self::Smb => 445,
            Self::Sftp => 22,
            Self::Ftp | Self::Ftps => 21,
        }
    }
}

/// Authentication mechanism used by SSH / SFTP connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshAuthMode {
    #[default]
    Password,
    PrivateKey,
}

impl SshAuthMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Password => "Password",
            Self::PrivateKey => "SSH Key",
        }
    }
}

/// Saved network server or shared folder profile persisted in `settings.json`.
///
/// Passwords and passphrases are NEVER stored inside this struct; when `save_password` is enabled,
/// secrets are stored encrypted in the OS credential store (Windows Credential Manager).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConnection {
    pub id: Uuid,
    pub name: String,
    pub protocol: NetworkProtocol,
    /// For SMB: UNC path (`\\server\share`) or hostname/share. For SFTP/FTP/FTPS: hostname or IP.
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Remote root/starting directory (e.g. `"/"` or `"/home/user"`). Ignored for SMB UNC roots.
    pub remote_path: String,
    pub auth_mode: SshAuthMode,
    pub private_key_path: Option<PathBuf>,
    /// When `true` on SFTP connections, launches `sftp-server` via `sudo` so root-owned paths
    /// such as `/root` and `/etc` can be browsed without enabling direct root SSH login.
    pub sftp_sudo: bool,
    pub save_password: bool,
}

impl Default for NetworkConnection {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            protocol: NetworkProtocol::Smb,
            host: String::new(),
            port: NetworkProtocol::Smb.default_port(),
            username: String::new(),
            remote_path: "/".to_string(),
            auth_mode: SshAuthMode::Password,
            private_key_path: None,
            sftp_sudo: false,
            save_password: true,
        }
    }
}

impl NetworkConnection {
    /// Returns the display label shown in the sidebar and breadcrumbs.
    pub fn display_name(&self) -> String {
        let trimmed = self.name.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        let host = self.host.trim();
        if !host.is_empty() {
            return host.to_string();
        }
        self.protocol.label().to_string()
    }

    /// Returns the normalized UNC path (`\\server\share`) when `protocol == NetworkProtocol::Smb`.
    pub fn smb_unc_path(&self) -> Option<PathBuf> {
        if self.protocol != NetworkProtocol::Smb {
            return None;
        }
        normalize_smb_share_path(&self.host).map(PathBuf::from)
    }

    /// Returns the normalized remote directory path (`/` prefixed, no trailing slash except `/`).
    pub fn normalized_remote_path(&self) -> String {
        normalize_remote_path(&self.remote_path)
    }

    /// Formats a clean human-readable endpoint summary (e.g. `user@192.168.1.50:22`).
    pub fn endpoint_summary(&self) -> String {
        match self.protocol {
            NetworkProtocol::Smb => normalize_smb_share_path(&self.host)
                .unwrap_or_else(|| self.host.trim().to_string()),
            NetworkProtocol::Sftp | NetworkProtocol::Ftp | NetworkProtocol::Ftps => {
                let user_prefix = if self.username.trim().is_empty() {
                    String::new()
                } else {
                    format!("{}@", self.username.trim())
                };
                if self.port == self.protocol.default_port() || self.port == 0 {
                    format!("{user_prefix}{}", self.host.trim())
                } else {
                    format!("{user_prefix}{}:{}", self.host.trim(), self.port)
                }
            }
        }
    }
}

/// Normalizes a POSIX remote path so it always starts with `/` and has no trailing slash (except `/`).
pub fn normalize_remote_path(raw: &str) -> String {
    let replaced = raw.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for seg in replaced.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// Joins a child entry name onto a normalized remote directory path.
pub fn join_remote_path(parent: &str, name: &str) -> String {
    let clean_parent = normalize_remote_path(parent);
    let clean_name = name.trim().trim_matches(['/', '\\']);
    if clean_name.is_empty() {
        return clean_parent;
    }
    if clean_parent == "/" {
        format!("/{clean_name}")
    } else {
        format!("{clean_parent}/{clean_name}")
    }
}

/// Normalizes SMB share input (`smb://server/share`, `//server/share`, `\\server\share`, or `server\share`)
/// into standard Windows UNC form (`\\server\share`).
pub fn normalize_smb_share_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    let without_scheme = trimmed
        .strip_prefix("smb://")
        .or_else(|| trimmed.strip_prefix("SMB://"))
        .unwrap_or(trimmed);
    let backslashed = without_scheme.replace('/', "\\");
    let stripped = backslashed.trim_start_matches('\\').trim_end_matches('\\');
    if stripped.is_empty() {
        return None;
    }
    Some(format!(r"\\{stripped}"))
}

/// Parsed URI components from address-bar network shortcuts (`sftp://`, `ssh://`, `ftp://`, `ftps://`, `smb://`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedNetworkUri {
    pub protocol: NetworkProtocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub remote_path: String,
}

/// Parses network URI strings such as `sftp://alice@10.0.0.5:2222/var/www`, `ftp://files.local/pub`,
/// or `smb://nas/media`.
pub fn parse_network_uri(raw: &str) -> Option<ParsedNetworkUri> {
    let trimmed = raw.trim().trim_matches('"');
    let (scheme_raw, rest) = trimmed.split_once("://")?;
    let protocol = match scheme_raw.to_ascii_lowercase().as_str() {
        "smb" => NetworkProtocol::Smb,
        "sftp" | "ssh" | "scp" => NetworkProtocol::Sftp,
        "ftp" => NetworkProtocol::Ftp,
        "ftps" => NetworkProtocol::Ftps,
        _ => return None,
    };

    if protocol == NetworkProtocol::Smb {
        let unc = normalize_smb_share_path(trimmed)?;
        return Some(ParsedNetworkUri {
            protocol,
            host: unc,
            port: protocol.default_port(),
            username: String::new(),
            remote_path: "/".to_string(),
        });
    }

    let (authority, path_part) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (username, host_port) = match authority.rsplit_once('@') {
        Some((u, hp)) => (u.trim().to_string(), hp.trim()),
        None => (String::new(), authority.trim()),
    };
    if host_port.is_empty() {
        return None;
    }

    let (host, port) = if host_port.starts_with('[') {
        if let Some((bracket_host, tail)) = host_port.split_once(']') {
            let p = tail
                .strip_prefix(':')
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or_else(|| protocol.default_port());
            (bracket_host.trim_start_matches('[').to_string(), p)
        } else {
            return None;
        }
    } else if let Some((h, p_str)) = host_port.rsplit_once(':')
        && let Ok(p) = p_str.parse::<u16>()
    {
        (h.to_string(), p)
    } else {
        (host_port.to_string(), protocol.default_port())
    };

    if host.is_empty() {
        return None;
    }

    Some(ParsedNetworkUri {
        protocol,
        host,
        port,
        username,
        remote_path: normalize_remote_path(path_part),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_remote_and_smb_paths() {
        assert_eq!(normalize_remote_path(""), "/");
        assert_eq!(normalize_remote_path("/var/www/"), "/var/www");
        assert_eq!(normalize_remote_path("a\\b/../c"), "/a/c");
        assert_eq!(join_remote_path("/", "logs"), "/logs");
        assert_eq!(join_remote_path("/var/www", "index.html"), "/var/www/index.html");

        assert_eq!(
            normalize_smb_share_path("smb://nas.local/Media/"),
            Some(r"\\nas.local\Media".to_string())
        );
        assert_eq!(
            normalize_smb_share_path(r"\\192.168.1.50\Share\"),
            Some(r"\\192.168.1.50\Share".to_string())
        );
    }

    #[test]
    fn parses_network_uris() {
        let sftp = parse_network_uri("sftp://admin@10.0.0.8:2222/srv/app/").unwrap();
        assert_eq!(sftp.protocol, NetworkProtocol::Sftp);
        assert_eq!(sftp.username, "admin");
        assert_eq!(sftp.host, "10.0.0.8");
        assert_eq!(sftp.port, 2222);
        assert_eq!(sftp.remote_path, "/srv/app");

        let ftp = parse_network_uri("ftp://ftp.example.com/pub").unwrap();
        assert_eq!(ftp.protocol, NetworkProtocol::Ftp);
        assert_eq!(ftp.port, 21);
        assert_eq!(ftp.remote_path, "/pub");

        let smb = parse_network_uri("smb://my-nas/backups").unwrap();
        assert_eq!(smb.protocol, NetworkProtocol::Smb);
        assert_eq!(smb.host, r"\\my-nas\backups");
    }
}
