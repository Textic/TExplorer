# TExplorer

Fast, minimalist, GPU-accelerated Windows file explorer built with **Rust (edition 2024)** and **GPUI (`gpui-kit`)**.

## Features (v0.6.0)

- **GPU-Accelerated Flat UI & Dynamic Themes:** Matte-black **Files Dark**, **Files Light**, automatic **System** mode (tracks Windows Dark/Light theme), and classic **Obsidian Dark** palettes with selectable accents (*Files Blue* default, *Files Cobalt*, *Files Mint*, *Obsidian Emerald*, *Obsidian Violet*, *Obsidian Mono*), `1px` panel cuts, zero shadows, crisp typography, categorized Settings, and a custom frameless titlebar.
- **Customizable Sidebar:** Collapsible, reorderable, and toggleable sidebar categories (*Quick Access*, *Pinned*, *Drives*, *Linux / WSL*), plus reorderable and toggleable *Quick Access* folders and flat compact drive rows.
- **Tabs, Dual-Pane Split View & Session Persistence:** Scrollable top tabs (`Ctrl+T` / `Ctrl+W`), independent per-tab Dual-Pane split (`Ctrl+\` or `F3`), mouse Back/Forward side-button navigation, auto-selecting address bar, virtualized **Details / Grid / Compact** views engineered for **100,000+ items** (off-thread sorting, zero-allocation filters, `O(1)` status bar), and automatic restoration of tabs, splits, and window bounds on startup.
- **Instant Global Search (`Ctrl+K`) & Live Folder Sizes (`texplorer-mft` / `texplorer-service`):** Optional read-only Windows service indexing NTFS `$MFT` in RAM and streaming real-time **USN Change Journal** deltas for `<10 ms` Everything-style queries (`ext:rs;toml`, `size:>10mb`, `dir:`, `file:`, `!exclude`, `/regex/`) and instant recursive folder sizes — with automatic multi-threaded `rayon` fallback when uninstalled or on non-NTFS drives.
- **Transparent Archive VFS (`.zip`, `.7z`, `.tar`, `.tar.gz`/`.tgz`):** Double-click archives to browse them like normal folders, preview images/Markdown/code live in Inspector or QuickLook without extracting, drag & drop items directly out of an archive into local folders or the opposite split pane, and compress local selections to `.zip`.
- **Power-User Arsenal:**
  - **Bulk Rename (`Ctrl+Shift+R`):** Literal or Regex (`$1`, `$2`) find/replace, templates (`{name}`, `{ext}`, `{n:03}`, `{date}`), casing rules, extension overrides, upfront collision detection, and atomic two-phase execution with full **Undo/Redo** (`Ctrl+Z` / `Ctrl+Y`).
  - **Command Palette (`Ctrl+Shift+P`):** Instant keyboard launcher for navigation, layout, archive, rename, checksum, file-lock, theme, and service actions.
  - **File Lock Detector & Streaming Checksums:** Windows **Restart Manager** handle inspection (process names & PIDs) and cancellable single-pass **SHA-256 & MD5** calculation with inline hash verification right inside the Inspector.
- **Async File Operations Engine (`texplorer-ops`):** Buffered `1 MiB` streaming copy/move queue with real-time EMA speed, auto-dismissing transfer drawer, interactive conflict resolution modal (*Overwrite*, *Skip*, *Keep Both*, *Apply to all*), atomic same-volume moves, and **Undo/Redo**.
- **Native Windows Shell Integration:** Interoperable `CF_HDROP` clipboard, dedicated COM STA worker for Windows thumbnails (`IShellItemImageFactory`), on-demand native Windows Shell menu (`Shift+Right Click`), internal/external Drag & Drop, and automatic WSL distribution discovery.
- **Safe Windows Default File Manager Integration:** Reversible per-user `HKCU\Software\Classes` registration (zero UAC), optional `Win+E` routing, Single-Instance Named Pipe tab handoff, automatic `explorer.exe` passthrough for system CLSIDs/Control Panel, one-click restore in Settings or via `texplorer --restore-explorer`, and automatic restoration on uninstall.
- **Windows Installer & Auto-Updater:** Inno Setup installer bundling `texplorer.exe` and optional `texplorer-service.exe` (with opt-in Default Explorer, `Win+E`, and MFT Service tasks) plus built-in GitHub Releases auto-updater with `SHA256SUMS.txt` and Windows PE signature checks.

## Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `Ctrl+K` | Global File Search (MFT Instant / Parallel Fallback) |
| `Ctrl+Shift+P` | Open Command Palette |
| `Ctrl+Shift+R` | Bulk Rename selected items |
| `Space` | Open / Close QuickLook preview (`↑`/`↓` to step files) |
| `Ctrl+T` / `Ctrl+W` | New / Close tab |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / Previous tab |
| `Ctrl+\` or `F3` | Toggle Dual-Pane Split |
| `Tab` | Switch active split pane |
| `Ctrl+L` / `Alt+D` | Edit address bar |
| `Alt+Left` / `Alt+Right` / `Alt+Up` | Back / Forward / Up |
| `Ctrl+F` | Focus inline folder filter |
| `Ctrl+B` / `Alt+P` | Toggle Sidebar / Preview & Inspector |
| `Ctrl+H` | Toggle hidden files |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / Cut / Paste (`CF_HDROP`) |
| `Ctrl+Z` / `Ctrl+Y` | Undo / Redo file operation (including Bulk Rename) |
| `Ctrl+Shift+N` / `F2` | New folder / Rename |
| `Del` / `Shift+Del` | Move to Recycle Bin / Delete permanently |
| `Shift+Right Click` | Open native Windows Shell context menu |
| `Ctrl+,` | Open Settings |

## Command-Line Flags

| Flag | Behavior |
| :--- | :--- |
| `texplorer.exe <path>` | Open folder in running instance (or start new window) |
| `texplorer.exe --select <file>` | Open parent folder and highlight `<file>` |
| `texplorer.exe --new-window` | Open an independent window (skips single-instance handoff) |
| `texplorer.exe --enable-default-explorer` | Register TExplorer as default per-user file manager (`HKCU`) |
| `texplorer.exe --enable-default-explorer-win-e` | Register as default file manager and intercept `Win+E` |
| `texplorer.exe --restore-explorer` | Restore original Windows Explorer associations immediately |

## Building from Source

```powershell
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release -p texplorer-app -p texplorer-service --locked
```

## License

MIT — see [LICENSE](LICENSE).
