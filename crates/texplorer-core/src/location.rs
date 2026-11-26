use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Scope for virtual search queries.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SearchScope {
    /// Search across all indexed local volumes.
    Global,
    /// Search within a specific local directory subtree.
    Folder(PathBuf),
}

/// Unified navigable location across local directories, Home dashboard,
/// virtual archive interiors, and search results.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Location {
    /// "This PC" / Home overview showing quick access, pinned folders, and drives.
    #[default]
    Home,
    /// Native filesystem path (local drive, UNC share, or WSL path).
    Local(PathBuf),
    /// Virtual path inside an archive file (`.zip`, `.7z`, `.tar`).
    Archive {
        archive_path: PathBuf,
        /// Forward-slash normalized inner path without leading slash.
        inner_path: String,
    },
    /// Virtual search results view.
    Search {
        query: String,
        scope: SearchScope,
    },
}

/// Clickable segment displayed in the pane breadcrumb bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreadcrumbSegment {
    pub label: String,
    pub target: Location,
}

impl Location {
    /// Constructs a normalized local location.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local(normalize_local_path(&path.into()))
    }

    /// Constructs a normalized archive location.
    pub fn archive(archive_path: impl Into<PathBuf>, inner_path: impl AsRef<str>) -> Self {
        Self::Archive {
            archive_path: normalize_local_path(&archive_path.into()),
            inner_path: normalize_archive_inner(inner_path.as_ref()),
        }
    }

    /// Returns the underlying local path when this location points directly to disk.
    pub fn as_local_path(&self) -> Option<&Path> {
        match self {
            Self::Local(p) => Some(p.as_path()),
            _ => None,
        }
    }

    /// Returns the human-readable path string for the editable address bar.
    pub fn display_path(&self) -> String {
        match self {
            Self::Home => "Home".to_string(),
            Self::Local(p) => p.to_string_lossy().into_owned(),
            Self::Archive {
                archive_path,
                inner_path,
            } => {
                if inner_path.is_empty() {
                    archive_path.to_string_lossy().into_owned()
                } else {
                    format!("{}\\{}", archive_path.display(), inner_path.replace('/', "\\"))
                }
            }
            Self::Search { query, scope } => match scope {
                SearchScope::Global => format!("search://{query}"),
                SearchScope::Folder(dir) => format!("search://{}?in={}", query, dir.display()),
            },
        }
    }

    /// Returns a concise label suitable for a browser tab title.
    pub fn tab_title(&self) -> String {
        match self {
            Self::Home => "Home".to_string(),
            Self::Local(path) => {
                if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && !name.is_empty()
                {
                    return name.to_string();
                }
                let raw = path.to_string_lossy();
                let trimmed = raw.trim_end_matches(['\\', '/']);
                if trimmed.is_empty() {
                    raw.into_owned()
                } else {
                    trimmed.to_string()
                }
            }
            Self::Archive {
                archive_path,
                inner_path,
            } => {
                if let Some(last) = inner_path.rsplit('/').next()
                    && !last.is_empty()
                {
                    return last.to_string();
                }
                archive_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Archive")
                    .to_string()
            }
            Self::Search { query, .. } => {
                if query.trim().is_empty() {
                    "Search".to_string()
                } else {
                    format!("Search: {}", query.trim())
                }
            }
        }
    }

    /// Computes the parent location when navigating "Up" (`Alt+Up`).
    pub fn parent(&self) -> Option<Self> {
        match self {
            Self::Home => None,
            Self::Local(path) => {
                if let Some(parent) = path.parent()
                    && !parent.as_os_str().is_empty()
                    && parent != path
                {
                    return Some(Self::local(parent));
                }
                Some(Self::Home)
            }
            Self::Archive {
                archive_path,
                inner_path,
            } => {
                if inner_path.is_empty() {
                    archive_path
                        .parent()
                        .map(Self::local)
                        .or(Some(Self::Home))
                } else if let Some((head, _)) = inner_path.rsplit_once('/') {
                    Some(Self::archive(archive_path.clone(), head))
                } else {
                    Some(Self::archive(archive_path.clone(), ""))
                }
            }
            Self::Search { scope, .. } => match scope {
                SearchScope::Global => Some(Self::Home),
                SearchScope::Folder(dir) => Some(Self::local(dir)),
            },
        }
    }

    /// Builds clickable breadcrumb segments for the address bar.
    pub fn breadcrumbs(&self) -> Vec<BreadcrumbSegment> {
        let mut out = vec![BreadcrumbSegment {
            label: "Home".to_string(),
            target: Self::Home,
        }];

        match self {
            Self::Home => {}
            Self::Local(path) => {
                let mut acc = PathBuf::new();
                for comp in path.components() {
                    match comp {
                        Component::Prefix(prefix) => {
                            acc.push(prefix.as_os_str());
                            let mut target = acc.clone();
                            target.push(std::path::MAIN_SEPARATOR_STR);
                            let label = prefix
                                .as_os_str()
                                .to_string_lossy()
                                .trim_end_matches(['\\', '/'])
                                .to_string();
                            out.push(BreadcrumbSegment {
                                label,
                                target: Self::local(target),
                            });
                        }
                        Component::RootDir => {
                            acc.push(comp.as_os_str());
                            if out.len() == 1 {
                                out.push(BreadcrumbSegment {
                                    label: "/".to_string(),
                                    target: Self::local(&acc),
                                });
                            }
                        }
                        Component::Normal(part) => {
                            acc.push(part);
                            out.push(BreadcrumbSegment {
                                label: part.to_string_lossy().into_owned(),
                                target: Self::local(&acc),
                            });
                        }
                        Component::CurDir | Component::ParentDir => {}
                    }
                }
            }
            Self::Archive {
                archive_path,
                inner_path,
            } => {
                out = Self::local(archive_path).breadcrumbs();
                if let Some(last) = out.last_mut() {
                    last.target = Self::archive(archive_path.clone(), "");
                }
                let mut acc = String::new();
                for part in inner_path.split('/').filter(|s| !s.is_empty()) {
                    if !acc.is_empty() {
                        acc.push('/');
                    }
                    acc.push_str(part);
                    out.push(BreadcrumbSegment {
                        label: part.to_string(),
                        target: Self::archive(archive_path.clone(), &acc),
                    });
                }
            }
            Self::Search { query, .. } => {
                out.push(BreadcrumbSegment {
                    label: format!("Search: {query}"),
                    target: self.clone(),
                });
            }
        }

        out
    }

    /// Parses user-entered text from the editable address bar into a `Location`.
    /// Supports `Home`, `This PC`, environment variables (`%USERPROFILE%`, `~`),
    /// bare drive letters (`C:`), and standard local/UNC paths.
    pub fn parse_user_input(raw: &str) -> Option<Self> {
        let trimmed = raw.trim().trim_matches('"');
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.eq_ignore_ascii_case("home") || trimmed.eq_ignore_ascii_case("this pc") {
            return Some(Self::Home);
        }

        let expanded = expand_path_shortcuts(trimmed);
        if expanded.len() == 2 && expanded.as_bytes()[1] == b':' {
            return Some(Self::local(format!("{expanded}\\")));
        }
        Some(Self::local(expanded))
    }
}

/// Normalizes a filesystem path by removing redundant trailing separators (except drive roots).
pub fn normalize_local_path(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    #[cfg(target_os = "windows")]
    let raw = raw.replace('/', "\\");

    // Preserve drive roots such as `C:\` and UNC prefixes.
    let bytes = raw.as_bytes();
    if bytes.len() == 2 && bytes[1] == b':' {
        return PathBuf::from(format!("{raw}\\"));
    }
    if bytes.len() == 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        let drive = (bytes[0] as char).to_ascii_uppercase();
        return PathBuf::from(format!("{drive}:\\"));
    }

    let trimmed = if raw.starts_with(r"\\") {
        let rest = raw.trim_end_matches('\\');
        if rest.len() <= 2 {
            raw.to_string()
        } else {
            rest.to_string()
        }
    } else if raw.len() > 1 {
        raw.trim_end_matches(['\\', '/']).to_string()
    } else {
        raw.to_string()
    };

    if trimmed.len() == 2 && trimmed.as_bytes()[1] == b':' {
        let drive = (trimmed.as_bytes()[0] as char).to_ascii_uppercase();
        return PathBuf::from(format!("{drive}:\\"));
    }

    // Uppercase Windows drive letter for consistent cache keys and display.
    let mut chars = trimmed.chars();
    if let (Some(first), Some(':')) = (chars.next(), chars.next())
        && first.is_ascii_alphabetic()
    {
        let mut normalized = String::with_capacity(trimmed.len());
        normalized.push(first.to_ascii_uppercase());
        normalized.push(':');
        normalized.push_str(chars.as_str());
        return PathBuf::from(normalized);
    }

    PathBuf::from(trimmed)
}

/// Normalizes an archive inner path to use `/` separators with no leading/trailing slash.
pub fn normalize_archive_inner(inner: &str) -> String {
    inner
        .replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn expand_path_shortcuts(input: &str) -> String {
    if (input == "~" || input.starts_with("~/") || input.starts_with("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        let suffix = input.trim_start_matches('~').trim_start_matches(['/', '\\']);
        return if suffix.is_empty() {
            home.to_string_lossy().into_owned()
        } else {
            home.join(suffix).to_string_lossy().into_owned()
        };
    }

    // Expand Windows-style %VAR% placeholders when present.
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('%') {
            let var_name = &after[..end];
            if !var_name.is_empty()
                && let Ok(val) = std::env::var(var_name)
            {
                out.push_str(&val);
                rest = &after[end + 1..];
                continue;
            }
            out.push('%');
            out.push_str(var_name);
            out.push('%');
            rest = &after[end + 1..];
        } else {
            out.push('%');
            rest = after;
            break;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_windows_paths_and_drive_roots() {
        assert_eq!(normalize_local_path(Path::new("c:")), PathBuf::from("C:\\"));
        assert_eq!(
            normalize_local_path(Path::new("x:\\TExplorer\\")),
            PathBuf::from("X:\\TExplorer")
        );
        assert_eq!(
            normalize_local_path(Path::new("d:/Projects/Rust/")),
            PathBuf::from("D:\\Projects\\Rust")
        );
    }

    #[test]
    fn builds_breadcrumbs_and_parents() {
        let loc = Location::local(r"X:\Projects\TExplorer");
        assert_eq!(loc.tab_title(), "TExplorer");

        let crumbs = loc.breadcrumbs();
        assert_eq!(crumbs.len(), 4);
        assert_eq!(crumbs[0].label, "Home");
        assert_eq!(crumbs[1].label, "X:");
        assert_eq!(crumbs[2].label, "Projects");
        assert_eq!(crumbs[3].label, "TExplorer");

        assert_eq!(loc.parent(), Some(Location::local(r"X:\Projects")));
        assert_eq!(
            Location::local(r"X:\").parent(),
            Some(Location::Home)
        );
        assert_eq!(Location::Home.parent(), None);
    }

    #[test]
    fn normalizes_archive_locations_and_parents() {
        let arch = Location::archive(r"X:\bundle.zip", r"\src\ui\mod.rs\");
        assert_eq!(arch.tab_title(), "mod.rs");
        assert_eq!(
            arch.parent(),
            Some(Location::archive(r"X:\bundle.zip", "src/ui"))
        );
        let root_arch = Location::archive(r"X:\bundle.zip", "");
        assert_eq!(root_arch.parent(), Some(Location::local(r"X:\")));
    }

    #[test]
    fn parses_user_address_bar_input() {
        assert_eq!(Location::parse_user_input("Home"), Some(Location::Home));
        assert_eq!(Location::parse_user_input("this pc"), Some(Location::Home));
        assert_eq!(
            Location::parse_user_input("c:"),
            Some(Location::local(r"C:\"))
        );
        assert_eq!(Location::parse_user_input("   "), None);
    }
}
