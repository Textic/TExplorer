use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO_URL: &str = "https://github.com/Textic/TExplorer";
const LATEST_RELEASE_API_URL: &str = "https://api.github.com/repos/Textic/TExplorer/releases/latest";
const LATEST_RELEASE_WEB_URL: &str = "https://github.com/Textic/TExplorer/releases/latest";
const SHA256SUMS_ASSET: &str = "SHA256SUMS.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallTargetKind {
    WindowsSetupInstaller,
    WindowsPortableExe,
    ManualBrowserFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub tag_name: String,
    pub release_url: String,
    pub download_url: String,
    pub asset_name: String,
    pub checksum_url: Option<String>,
    pub published_at: String,
    pub install_kind: InstallTargetKind,
}

impl UpdateInfo {
    pub fn supports_in_app_install(&self) -> bool {
        self.install_kind != InstallTargetKind::ManualBrowserFallback
            && self.download_url.starts_with("https://")
            && !self.asset_name.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading {
        info: UpdateInfo,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Installing(UpdateInfo),
    Failed(String),
}

impl UpdateState {
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing(_)
        )
    }
}

#[derive(Debug, Clone)]
pub struct StagedUpdate {
    pub kind: InstallTargetKind,
    pub version_label: String,
    pub target_exe: PathBuf,
    pub source_path: PathBuf,
    pub stage_dir: PathBuf,
}

pub fn format_download_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else if bytes as f64 >= KB {
        format!("{} KB", (bytes as f64 / KB).round() as u64)
    } else {
        format!("{bytes} B")
    }
}

fn hex_encode_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

pub fn parse_sha256_for_asset(sums_text: &str, asset_name: &str) -> Option<String> {
    if asset_name.is_empty() {
        return None;
    }
    for line in sums_text.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(raw_name) = parts.next() else {
            continue;
        };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let cleaned = raw_name.trim_start_matches('*').trim_start_matches("./");
        let basename = cleaned.rsplit(['/', '\\']).next().unwrap_or(cleaned);
        if cleaned == asset_name || basename == asset_name {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

pub fn detect_install_kind_for(
    os: &str,
    has_windows_uninstaller: bool,
    target_dir_writable: bool,
) -> InstallTargetKind {
    if os == "windows" {
        if has_windows_uninstaller {
            InstallTargetKind::WindowsSetupInstaller
        } else if target_dir_writable {
            InstallTargetKind::WindowsPortableExe
        } else {
            InstallTargetKind::ManualBrowserFallback
        }
    } else {
        InstallTargetKind::ManualBrowserFallback
    }
}

pub fn detect_install_kind() -> InstallTargetKind {
    let Ok(exe_path) = std::env::current_exe() else {
        return InstallTargetKind::ManualBrowserFallback;
    };
    let os = std::env::consts::OS;
    let exe_dir = exe_path.parent();
    let has_win_uninstaller = os == "windows"
        && exe_dir
            .map(|d| d.join("unins000.exe").is_file())
            .unwrap_or(false);
    let target_dir_writable = exe_dir.is_some_and(is_dir_writable);

    detect_install_kind_for(os, has_win_uninstaller, target_dir_writable)
}

fn is_dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe = dir.join(format!(
        ".texplorer-write-probe-{}-{stamp}",
        std::process::id()
    ));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn pick_asset_for_install<'a>(
    assets: &'a [(String, String)],
    preferred_kind: InstallTargetKind,
    os: &str,
) -> Option<(&'a str, &'a str, InstallTargetKind)> {
    let patterns: &[&str] = match (preferred_kind, os) {
        (InstallTargetKind::WindowsSetupInstaller, "windows") => {
            &["windows-x86_64-setup.exe", "windows-x86_64.exe", ".exe"]
        }
        (InstallTargetKind::WindowsPortableExe, "windows") => {
            &["windows-x86_64.exe", "windows-x86_64-setup.exe"]
        }
        (_, "windows") => &[
            "windows-x86_64-setup.exe",
            "windows-x86_64.exe",
            "windows-x86_64.zip",
            ".exe",
        ],
        _ => &[],
    };

    for pat in patterns {
        if let Some((name, url)) = assets.iter().find(|(n, _)| {
            let lower = n.to_ascii_lowercase();
            lower.ends_with(pat) && (pat.contains(os) || lower.contains(os))
        }) {
            let effective_kind = resolve_install_kind_for_asset(name, preferred_kind, os);
            return Some((name.as_str(), url.as_str(), effective_kind));
        }
    }
    None
}

fn resolve_install_kind_for_asset(
    asset_name: &str,
    preferred: InstallTargetKind,
    os: &str,
) -> InstallTargetKind {
    let lower = asset_name.to_ascii_lowercase();
    if os == "windows" {
        if lower.ends_with("-setup.exe") {
            InstallTargetKind::WindowsSetupInstaller
        } else if lower.ends_with(".exe") {
            if preferred == InstallTargetKind::ManualBrowserFallback {
                InstallTargetKind::ManualBrowserFallback
            } else {
                InstallTargetKind::WindowsPortableExe
            }
        } else {
            InstallTargetKind::ManualBrowserFallback
        }
    } else {
        InstallTargetKind::ManualBrowserFallback
    }
}

pub async fn download_and_stage_update<F>(
    info: &UpdateInfo,
    mut on_progress: F,
) -> Result<StagedUpdate, String>
where
    F: FnMut(u64, Option<u64>) + Send,
{
    if info.install_kind == InstallTargetKind::ManualBrowserFallback {
        return Err(
            "Automatic install is unavailable for this installation; please download manually."
                .to_string(),
        );
    }
    if !info.download_url.starts_with("https://") {
        return Err("Refusing to download update over an insecure URL.".to_string());
    }

    let target_exe = std::env::current_exe()
        .map_err(|e| format!("Could not resolve current executable path: {e}"))?;
    preflight_target_writable(&target_exe, info.install_kind)?;

    let stage_dir = prepare_staging_dir(&info.tag_name)?;
    let client = reqwest::Client::builder()
        .user_agent(format!("TExplorer/{CURRENT_VERSION}"))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let expected_sha256 = fetch_expected_sha256(&client, info).await?;

    let safe_filename = info
        .asset_name
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("texplorer-update.exe");
    let download_path = stage_dir.join(safe_filename);

    let actual_sha256 =
        stream_asset_with_hash(&client, &info.download_url, &download_path, &mut on_progress)
            .await
            .inspect_err(|_| {
                let _ = fs::remove_dir_all(&stage_dir);
            })?;

    if let Some(expected) = expected_sha256
        && actual_sha256 != expected
    {
        let _ = fs::remove_dir_all(&stage_dir);
        return Err(format!(
            "SHA-256 verification failed for {} (expected {expected}, got {actual_sha256})",
            info.asset_name
        ));
    }

    verify_windows_pe_header(&download_path).inspect_err(|_| {
        let _ = fs::remove_dir_all(&stage_dir);
    })?;

    Ok(StagedUpdate {
        kind: info.install_kind,
        version_label: info.tag_name.clone(),
        target_exe,
        source_path: download_path,
        stage_dir,
    })
}

fn preflight_target_writable(target_exe: &Path, kind: InstallTargetKind) -> Result<(), String> {
    match kind {
        InstallTargetKind::WindowsSetupInstaller => Ok(()),
        InstallTargetKind::WindowsPortableExe => {
            let dir = target_exe
                .parent()
                .ok_or_else(|| "Current executable has no parent directory.".to_string())?;
            if !is_dir_writable(dir) {
                return Err(format!(
                    "Target folder \"{}\" is not writable; download manually from Releases.",
                    dir.display()
                ));
            }
            Ok(())
        }
        InstallTargetKind::ManualBrowserFallback => {
            Err("Automatic installation is unavailable for this installation.".to_string())
        }
    }
}

fn updates_root_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("TExplorer")
        .join("updates")
}

fn prepare_staging_dir(tag_name: &str) -> Result<PathBuf, String> {
    let root = updates_root_dir();
    fs::create_dir_all(&root).map_err(|e| format!("Failed to create updates cache dir: {e}"))?;

    if let Ok(entries) = fs::read_dir(&root) {
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let is_stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|mtime| now.duration_since(mtime).ok())
                .is_some_and(|age| age > Duration::from_secs(3600));
            if is_stale {
                let _ = fs::remove_dir_all(path);
            }
        }
    }

    let safe_tag: String = tag_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let stage = root.join(format!("stage-{safe_tag}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&stage).map_err(|e| format!("Failed to create staging dir: {e}"))?;
    Ok(stage)
}

async fn fetch_expected_sha256(
    client: &reqwest::Client,
    info: &UpdateInfo,
) -> Result<Option<String>, String> {
    let Some(sum_url) = info.checksum_url.as_deref() else {
        return Ok(None);
    };
    if !sum_url.starts_with("https://") {
        return Err("Refusing checksum download over an insecure URL.".to_string());
    }
    let resp = client
        .get(sum_url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch SHA256SUMS.txt: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "Failed to fetch SHA256SUMS.txt (HTTP {})",
            resp.status().as_u16()
        ));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read SHA256SUMS.txt: {e}"))?;
    let expected = parse_sha256_for_asset(&text, &info.asset_name).ok_or_else(|| {
        format!(
            "SHA256SUMS.txt did not contain an entry for {}",
            info.asset_name
        )
    })?;
    Ok(Some(expected))
}

async fn stream_asset_with_hash<F>(
    client: &reqwest::Client,
    url: &str,
    dest_path: &Path,
    on_progress: &mut F,
) -> Result<String, String>
where
    F: FnMut(u64, Option<u64>),
{
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Failed to download update: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "Update download failed with HTTP {}",
            resp.status().as_u16()
        ));
    }

    let total_bytes = resp.content_length();
    on_progress(0, total_bytes);

    let mut file = tokio::fs::File::create(dest_path)
        .await
        .map_err(|e| format!("Failed to create temporary installer file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now();
    let mut stream = resp.bytes_stream();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| format!("Download interrupted: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Failed writing update file: {e}"))?;
        hasher.update(&chunk);
        downloaded = downloaded.saturating_add(chunk.len() as u64);

        if last_emit.elapsed() >= Duration::from_millis(80) {
            on_progress(downloaded, total_bytes);
            last_emit = Instant::now();
        }
    }

    file.flush()
        .await
        .map_err(|e| format!("Failed flushing update file: {e}"))?;
    file.sync_all()
        .await
        .map_err(|e| format!("Failed syncing update file: {e}"))?;
    on_progress(downloaded, total_bytes.or(Some(downloaded)));

    Ok(hex_encode_lower(&hasher.finalize()))
}

fn verify_windows_pe_header(path: &Path) -> Result<(), String> {
    use std::io::Read;
    let mut f = fs::File::open(path).map_err(|e| format!("Cannot read downloaded file: {e}"))?;
    let mut magic = [0u8; 2];
    f.read_exact(&mut magic)
        .map_err(|_| "Downloaded Windows executable is truncated or invalid.".to_string())?;
    if &magic != b"MZ" {
        return Err("Downloaded file is not a valid Windows executable.".to_string());
    }
    Ok(())
}

pub fn spawn_detached_updater(staged: &StagedUpdate, dark_mode: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        spawn_windows_updater(staged, dark_mode)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (staged, dark_mode);
        Err("In-app self-update is currently supported on Windows only.".to_string())
    }
}

#[cfg(target_os = "windows")]
const WIN_CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(target_os = "windows")]
const WIN_CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

#[cfg(target_os = "windows")]
fn spawn_windows_updater(staged: &StagedUpdate, dark_mode: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let script_path = staged.stage_dir.join("apply-update.ps1");
    let log_path = updates_root_dir().join("last-update.log");
    let install_dir_path = staged
        .target_exe
        .parent()
        .ok_or_else(|| "Current executable has no parent directory.".to_string())?;
    let install_dir = install_dir_path
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_string();

    let mode = match staged.kind {
        InstallTargetKind::WindowsSetupInstaller => "setup",
        InstallTargetKind::WindowsPortableExe => "portable",
        _ => return Err("Unsupported Windows self-update mode.".to_string()),
    };

    let dir_writable = is_dir_writable(install_dir_path);
    let is_program_files = install_dir.to_ascii_lowercase().contains("\\program files");
    let all_users = if !dir_writable || is_program_files {
        "1"
    } else {
        "0"
    };
    let needs_elevation = if !dir_writable && !texplorer_platform::is_elevated() {
        "1"
    } else {
        "0"
    };

    let ps1 = r#"$ErrorActionPreference = 'Stop'
$pidToWait = [int]$env:TEXPLORER_PID
$targetExe = $env:TEXPLORER_TARGET_EXE
$sourceFile = $env:TEXPLORER_SOURCE_FILE
$installDir = $env:TEXPLORER_INSTALL_DIR
$stageDir = $env:TEXPLORER_STAGE_DIR
$mode = $env:TEXPLORER_MODE
$allUsers = $env:TEXPLORER_ALL_USERS
$needsElevation = $env:TEXPLORER_NEEDS_ELEVATION
$logFile = $env:TEXPLORER_LOG_FILE
$newVersion = if ($env:TEXPLORER_NEW_VERSION) { $env:TEXPLORER_NEW_VERSION } else { 'latest' }
$isDark = ($env:TEXPLORER_DARK_MODE -ne '0')

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$bgColor      = if ($isDark) { [System.Drawing.Color]::FromArgb(24, 24, 27) } else { [System.Drawing.Color]::FromArgb(250, 250, 250) }
$borderColor  = if ($isDark) { [System.Drawing.Color]::FromArgb(46, 46, 52) } else { [System.Drawing.Color]::FromArgb(220, 220, 226) }
$fgPrimary    = if ($isDark) { [System.Drawing.Color]::FromArgb(244, 244, 246) } else { [System.Drawing.Color]::FromArgb(24, 24, 27) }
$fgSecondary  = if ($isDark) { [System.Drawing.Color]::FromArgb(161, 161, 170) } else { [System.Drawing.Color]::FromArgb(100, 100, 110) }
$trackColor   = if ($isDark) { [System.Drawing.Color]::FromArgb(42, 42, 48) } else { [System.Drawing.Color]::FromArgb(228, 228, 232) }
$accentColor  = if ($isDark) { [System.Drawing.Color]::FromArgb(80, 192, 255) } else { [System.Drawing.Color]::FromArgb(0, 112, 203) }
$errorColor   = [System.Drawing.Color]::FromArgb(239, 68, 68)

$form = New-Object System.Windows.Forms.Form
$form.Text = "Updating TExplorer"
$form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::None
$form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
$form.ClientSize = New-Object System.Drawing.Size(430, 152)
$form.BackColor = $bgColor
$form.TopMost = $true
$form.ShowInTaskbar = $true

$form.Add_Paint({
    param($s, $e)
    $pen = New-Object System.Drawing.Pen($borderColor, 1)
    $e.Graphics.DrawRectangle($pen, 0, 0, $s.ClientSize.Width - 1, $s.ClientSize.Height - 1)
    $pen.Dispose()
})

$titleLbl = New-Object System.Windows.Forms.Label
$titleLbl.Text = "Updating TExplorer to $newVersion"
$titleLbl.Font = New-Object System.Drawing.Font("Segoe UI Semibold", 11.5)
$titleLbl.ForeColor = $fgPrimary
$titleLbl.Location = New-Object System.Drawing.Point(22, 20)
$titleLbl.Size = New-Object System.Drawing.Size(386, 26)
$form.Controls.Add($titleLbl)

$statusLbl = New-Object System.Windows.Forms.Label
$statusLbl.Text = "Closing TExplorer..."
$statusLbl.Font = New-Object System.Drawing.Font("Segoe UI", 9.5)
$statusLbl.ForeColor = $fgSecondary
$statusLbl.Location = New-Object System.Drawing.Point(22, 50)
$statusLbl.Size = New-Object System.Drawing.Size(386, 22)
$form.Controls.Add($statusLbl)

$trackPanel = New-Object System.Windows.Forms.Panel
$trackPanel.Location = New-Object System.Drawing.Point(22, 86)
$trackPanel.Size = New-Object System.Drawing.Size(386, 8)
$trackPanel.BackColor = $trackColor
$form.Controls.Add($trackPanel)

$fillPanel = New-Object System.Windows.Forms.Panel
$fillPanel.Location = New-Object System.Drawing.Point(0, 0)
$fillPanel.Size = New-Object System.Drawing.Size(24, 8)
$fillPanel.BackColor = $accentColor
$trackPanel.Controls.Add($fillPanel)

$detailLbl = New-Object System.Windows.Forms.Label
$detailLbl.Text = "TExplorer will reopen automatically once the update finishes."
$detailLbl.Font = New-Object System.Drawing.Font("Segoe UI", 8.5)
$detailLbl.ForeColor = $fgSecondary
$detailLbl.Location = New-Object System.Drawing.Point(22, 108)
$detailLbl.Size = New-Object System.Drawing.Size(386, 28)
$form.Controls.Add($detailLbl)

$closeBtn = New-Object System.Windows.Forms.Button
$closeBtn.Text = "Close"
$closeBtn.FlatStyle = [System.Windows.Forms.FlatStyle]::Flat
$closeBtn.FlatAppearance.BorderColor = $borderColor
$closeBtn.BackColor = $trackColor
$closeBtn.ForeColor = $fgPrimary
$closeBtn.Font = New-Object System.Drawing.Font("Segoe UI", 9)
$closeBtn.Size = New-Object System.Drawing.Size(80, 28)
$closeBtn.Location = New-Object System.Drawing.Point(328, 110)
$closeBtn.Visible = $false
$closeBtn.Add_Click({ $form.Close() })
$form.Controls.Add($closeBtn)

function Set-ProgressUi([string]$msg, [int]$pct) {
    if ($pct -lt 0) { $pct = 0 }
    if ($pct -gt 100) { $pct = 100 }
    $statusLbl.Text = $msg
    $fillPanel.Width = [int]([Math]::Round(386.0 * ($pct / 100.0)))
    [System.Windows.Forms.Application]::DoEvents()
}

function Pump-Sleep([int]$ms) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        [System.Windows.Forms.Application]::DoEvents()
        Start-Sleep -Milliseconds 30
    }
}

$form.Show()
$form.Activate()
[System.Windows.Forms.Application]::DoEvents()

try {
    Set-ProgressUi "Closing TExplorer..." 12
    for ($i = 0; $i -lt 150; $i++) {
        if (-not (Get-Process -Id $pidToWait -ErrorAction SilentlyContinue)) {
            break
        }
        Pump-Sleep 150
    }
    Pump-Sleep 250

    if ($mode -eq 'setup') {
        Set-ProgressUi "Installing TExplorer $newVersion..." 30
        $scopeArg = if ($allUsers -eq '1') { '/ALLUSERS' } else { '/CURRENTUSER' }
        $argsLine = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /SP- ' + $scopeArg + ' /DIR="' + $installDir + '"'
        if ($needsElevation -eq '1') {
            $detailLbl.Text = "Waiting for administrator approval (UAC) and installer..."
            [System.Windows.Forms.Application]::DoEvents()
            $proc = Start-Process -FilePath $sourceFile -Verb RunAs -ArgumentList $argsLine -PassThru
        } else {
            $proc = Start-Process -FilePath $sourceFile -ArgumentList $argsLine -PassThru
        }
        # Touch Handle immediately so PowerShell 5.1 populates ExitCode reliably after exit.
        try { $null = $proc.Handle } catch {}

        $pct = 32.0
        while (-not $proc.HasExited) {
            if ($pct -lt 88.0) {
                $pct = [Math]::Min(88.0, $pct + 1.2)
                Set-ProgressUi "Installing TExplorer $newVersion..." ([int]$pct)
            } else {
                [System.Windows.Forms.Application]::DoEvents()
            }
            Start-Sleep -Milliseconds 120
        }
        try { $proc.Refresh() } catch {}
        $exitCode = $proc.ExitCode
        if ($null -ne $exitCode -and $exitCode -ne 0) {
            throw "Installer exited with code $exitCode"
        }
    } else {
        Set-ProgressUi "Replacing TExplorer executable..." 45
        $copied = $false
        for ($attempt = 0; $attempt -lt 30; $attempt++) {
            try {
                Copy-Item -LiteralPath $sourceFile -Destination $targetExe -Force
                $copied = $true
                break
            } catch {
                Set-ProgressUi "Waiting for file lock to release..." ([Math]::Min(82, 45 + $attempt))
                Pump-Sleep 180
            }
        }
        if (-not $copied) {
            throw "Failed to replace $targetExe"
        }
    }

    Set-ProgressUi "Launching TExplorer $newVersion..." 95
    $detailLbl.Text = "Update complete - opening TExplorer..."
    [System.Windows.Forms.Application]::DoEvents()

    $launched = $null
    if (Test-Path -LiteralPath $targetExe) {
        $launched = Start-Process -FilePath $targetExe -WorkingDirectory $installDir -PassThru
    } else {
        $fallbackExe = Join-Path $installDir 'texplorer.exe'
        if (Test-Path -LiteralPath $fallbackExe) {
            $launched = Start-Process -FilePath $fallbackExe -WorkingDirectory $installDir -PassThru
        }
    }

    Set-ProgressUi "TExplorer $newVersion is ready!" 100
    if ($launched) {
        for ($w = 0; $w -lt 40; $w++) {
            try {
                $launched.Refresh()
                if ($launched.HasExited -or $launched.MainWindowHandle -ne [IntPtr]::Zero) {
                    break
                }
            } catch {
                break
            }
            Pump-Sleep 120
        }
    } else {
        Pump-Sleep 600
    }

    $form.TopMost = $false
    $form.Close()
    $form.Dispose()

    if ($stageDir -and (Test-Path -LiteralPath $stageDir)) {
        Remove-Item -LiteralPath $stageDir -Recurse -Force -ErrorAction SilentlyContinue
    }
} catch {
    $errMsg = $_.ToString()
    try {
        "$(Get-Date -Format o) UPDATE FAILED: $errMsg" | Out-File -LiteralPath $logFile -Encoding utf8
    } catch {}
    $titleLbl.Text = "TExplorer update could not complete"
    $statusLbl.Text = $errMsg
    $statusLbl.ForeColor = $errorColor
    $fillPanel.BackColor = $errorColor
    $fillPanel.Width = 386
    $detailLbl.Text = "Log saved to: $logFile"
    $detailLbl.Size = New-Object System.Drawing.Size(296, 32)
    $closeBtn.Visible = $true
    [System.Windows.Forms.Application]::Run($form)
    exit 1
}
"#;
    fs::write(&script_path, ps1)
        .map_err(|e| format!("Failed to write update helper script: {e}"))?;

    Command::new("powershell.exe")
        .args([
            "-STA",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(&script_path)
        .env("TEXPLORER_PID", std::process::id().to_string())
        .env("TEXPLORER_TARGET_EXE", &staged.target_exe)
        .env("TEXPLORER_SOURCE_FILE", &staged.source_path)
        .env("TEXPLORER_INSTALL_DIR", install_dir)
        .env("TEXPLORER_STAGE_DIR", &staged.stage_dir)
        .env("TEXPLORER_MODE", mode)
        .env("TEXPLORER_ALL_USERS", all_users)
        .env("TEXPLORER_NEEDS_ELEVATION", needs_elevation)
        .env("TEXPLORER_LOG_FILE", log_path)
        .env("TEXPLORER_NEW_VERSION", &staged.version_label)
        .env("TEXPLORER_DARK_MODE", if dark_mode { "1" } else { "0" })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(WIN_CREATE_NO_WINDOW | WIN_CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map_err(|e| format!("Failed to launch background updater: {e}"))?;

    Ok(())
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

pub fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    let core = trimmed.split(['-', '+']).next()?.trim();
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer_version(current: &str, latest: &str) -> bool {
    match (parse_semver(current), parse_semver(latest)) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => false,
    }
}

/// Extracts the release tag name from a redirected GitHub release URL such as
/// `https://github.com/Textic/TExplorer/releases/tag/v0.6.1`.
pub fn extract_tag_from_release_url(url: &str) -> Option<String> {
    let (_, after_tag) = url.split_once("/releases/tag/")?;
    let tag = after_tag
        .split(['?', '#', '/'])
        .next()
        .unwrap_or("")
        .trim();
    if tag.is_empty() {
        None
    } else {
        Some(tag.to_string())
    }
}

/// Parses `(asset_name, download_url)` pairs from a release `SHA256SUMS.txt` file
/// for the given `tag_name`, avoiding the rate-limited GitHub REST API.
pub fn parse_assets_from_sha256sums(sums_text: &str, tag_name: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in sums_text.lines() {
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(raw_name) = parts.next() else {
            continue;
        };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let cleaned = raw_name.trim_start_matches('*').trim_start_matches("./");
        let basename = cleaned.rsplit(['/', '\\']).next().unwrap_or(cleaned).trim();
        if basename.is_empty() {
            continue;
        }
        let url = format!("{REPO_URL}/releases/download/{tag_name}/{basename}");
        pairs.push((basename.to_string(), url));
    }
    pairs
}

fn build_update_info(
    tag_name: String,
    release_url: String,
    published_at: String,
    pairs: &[(String, String)],
    checksum_url: Option<String>,
) -> UpdateInfo {
    let version = tag_name
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    let preferred_kind = detect_install_kind();
    let (asset_name, download_url, install_kind) =
        match pick_asset_for_install(pairs, preferred_kind, std::env::consts::OS) {
            Some((name, url, kind)) => (name.to_string(), url.to_string(), kind),
            None => (
                String::new(),
                release_url.clone(),
                InstallTargetKind::ManualBrowserFallback,
            ),
        };

    UpdateInfo {
        version,
        tag_name,
        release_url,
        download_url,
        asset_name,
        checksum_url,
        published_at,
        install_kind,
    }
}

/// Fallback update check using `github.com/.../releases/latest` HTTP redirect + `SHA256SUMS.txt`,
/// which does not consume GitHub's 60 req/hr unauthenticated REST API quota (HTTP 403 on shared IPs/CGNAT).
async fn check_for_updates_via_web_redirect(
    client: &reqwest::Client,
) -> Result<Option<UpdateInfo>, String> {
    let resp = client
        .get(LATEST_RELEASE_WEB_URL)
        .header("Accept", "text/html,application/xhtml+xml")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!(
            "GitHub Releases returned HTTP {}",
            resp.status().as_u16()
        ));
    }

    let final_url = resp.url().as_str().to_string();
    let Some(tag_name) = extract_tag_from_release_url(&final_url) else {
        return Ok(None);
    };

    if !is_newer_version(CURRENT_VERSION, &tag_name) {
        return Ok(None);
    }

    let release_url = format!("{REPO_URL}/releases/tag/{tag_name}");
    let checksum_candidate = format!("{REPO_URL}/releases/download/{tag_name}/{SHA256SUMS_ASSET}");

    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut checksum_url: Option<String> = None;

    if let Ok(sum_resp) = client
        .get(&checksum_candidate)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        && sum_resp.status().is_success()
        && let Ok(text) = sum_resp.text().await
    {
        pairs = parse_assets_from_sha256sums(&text, &tag_name);
        if !pairs.is_empty() {
            checksum_url = Some(checksum_candidate);
        }
    }

    // If SHA256SUMS.txt wasn't present on that release, still allow opening Releases in browser.
    Ok(Some(build_update_info(
        tag_name,
        release_url,
        String::new(),
        &pairs,
        checksum_url,
    )))
}

pub async fn check_for_updates() -> Result<Option<UpdateInfo>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(format!("TExplorer/{CURRENT_VERSION}"))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

    let resp = client
        .get(LATEST_RELEASE_API_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    if status == 404 {
        return Ok(None);
    }

    // Unauthenticated GitHub REST API is limited to 60 requests/hour per IP (often exhausted on
    // ISP CGNAT / shared Wi-Fi / VPNs -> HTTP 403/429). Seamlessly fall back to the non-API
    // web release redirect which has no REST API rate limit.
    if matches!(status, 403 | 429) || status >= 500 {
        return check_for_updates_via_web_redirect(&client).await.map_err(|fallback_err| {
            if status == 403 || status == 429 {
                format!("GitHub rate limit reached (HTTP {status}); {fallback_err}")
            } else {
                format!("GitHub API returned HTTP {status}; {fallback_err}")
            }
        });
    }

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned HTTP {status}"));
    }

    let rel: GithubRelease = resp
        .json()
        .await
        .map_err(|e| format!("Invalid GitHub release payload: {e}"))?;

    if rel.draft || rel.prerelease || !is_newer_version(CURRENT_VERSION, &rel.tag_name) {
        return Ok(None);
    }

    let pairs: Vec<(String, String)> = rel
        .assets
        .iter()
        .map(|a| (a.name.clone(), a.browser_download_url.clone()))
        .collect();
    let checksum_url = pairs
        .iter()
        .find(|(name, _)| name == SHA256SUMS_ASSET)
        .map(|(_, url)| url.clone());
    let published_at = rel
        .published_at
        .as_deref()
        .and_then(|s| s.split('T').next())
        .unwrap_or("")
        .to_string();

    Ok(Some(build_update_info(
        rel.tag_name,
        rel.html_url,
        published_at,
        &pairs,
        checksum_url,
    )))
}

pub fn open_url_in_browser(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("Refusing to open non-HTTP URL.".to_string());
    }

    #[cfg(target_os = "windows")]
    let status = Command::new("explorer.exe").arg(trimmed).spawn();
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(trimmed).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(trimmed).spawn();

    status
        .map(|_| ())
        .map_err(|e| format!("Failed to open browser: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_and_picks_windows_assets() {
        assert_eq!(parse_semver("v0.1.0"), Some((0, 1, 0)));
        assert!(is_newer_version("0.1.0", "v0.1.1"));
        assert!(!is_newer_version("0.1.0", "v0.1.0"));

        let assets = vec![
            (
                "texplorer-v0.1.1-windows-x86_64.exe".to_string(),
                "https://example.com/portable.exe".to_string(),
            ),
            (
                "texplorer-v0.1.1-windows-x86_64-setup.exe".to_string(),
                "https://example.com/setup.exe".to_string(),
            ),
        ];

        assert_eq!(
            pick_asset_for_install(&assets, InstallTargetKind::WindowsSetupInstaller, "windows"),
            Some((
                "texplorer-v0.1.1-windows-x86_64-setup.exe",
                "https://example.com/setup.exe",
                InstallTargetKind::WindowsSetupInstaller,
            ))
        );
        assert_eq!(
            pick_asset_for_install(&assets, InstallTargetKind::WindowsPortableExe, "windows"),
            Some((
                "texplorer-v0.1.1-windows-x86_64.exe",
                "https://example.com/portable.exe",
                InstallTargetKind::WindowsPortableExe,
            ))
        );
    }

    #[test]
    fn parses_sha256_entries() {
        let sums = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  texplorer-v0.1.1-windows-x86_64.exe\n";
        assert_eq!(
            parse_sha256_for_asset(sums, "texplorer-v0.1.1-windows-x86_64.exe"),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string())
        );
        assert_eq!(
            parse_sha256_for_asset(sums, "texplorer-v0.1.1-windows-x86_64-setup.exe"),
            None
        );
    }

    #[test]
    fn extracts_tag_and_assets_for_web_redirect_fallback() {
        assert_eq!(
            extract_tag_from_release_url(
                "https://github.com/Textic/TExplorer/releases/tag/v0.6.1"
            ),
            Some("v0.6.1".to_string())
        );
        assert_eq!(
            extract_tag_from_release_url(
                "https://github.com/Textic/TExplorer/releases/tag/v0.6.1?foo=bar"
            ),
            Some("v0.6.1".to_string())
        );
        assert_eq!(
            extract_tag_from_release_url("https://github.com/Textic/TExplorer/releases"),
            None
        );

        let sums = "\
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  texplorer-v0.6.1-windows-x86_64-setup.exe
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  texplorer-v0.6.1-windows-x86_64.exe
";
        let assets = parse_assets_from_sha256sums(sums, "v0.6.1");
        assert_eq!(assets.len(), 2);
        assert_eq!(
            assets[0],
            (
                "texplorer-v0.6.1-windows-x86_64-setup.exe".to_string(),
                "https://github.com/Textic/TExplorer/releases/download/v0.6.1/texplorer-v0.6.1-windows-x86_64-setup.exe".to_string()
            )
        );
    }
}
