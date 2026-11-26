# TExplorer

Fast, minimalist flat-dark Windows file explorer built with **Rust** and **GPUI (`gpui-kit`)**.

## Features (v0.2.0)

- **GPU-Accelerated Obsidian Dark UI:** Strict flat dark palette (`#09090B` / `#111113` / `#18181B` / `#27272A`) with `1px` panel cuts, zero shadows, selectable accents (*Emerald*, *Violet*, *Monochrome*), and a custom `38px` frameless titlebar.
- **Tabs & Dual-Pane Split View:** Optional top tab bar (`Settings → Top Tab Bar`) and independent per-tab Dual-Pane split (`Ctrl+\` or `F3`).
- **Async File Operations Engine (`texplorer-ops`):** Buffered `1 MiB` streaming copy/move queue with real-time EMA speed, pause/resume/cancel drawer, interactive conflict resolution modal (*Overwrite*, *Skip*, *Keep Both*, *Apply to all*), atomic same-volume moves, and **Undo/Redo** (`Ctrl+Z` / `Ctrl+Y`).
- **Native Windows Shell Integration:** Real `CF_HDROP` + `Preferred DropEffect` clipboard (`Ctrl+C`/`X`/`V` interoperable with Windows Explorer), dedicated COM STA worker for Windows thumbnails (`IShellItemImageFactory`) in Grid View, on-demand native Windows context menu (`Show Windows Menu` / `Shift+Right Click`), and internal/external Drag & Drop (`ExternalPaths`).
- **Instant Preview & Spacebar QuickLook:** Collapsible right Inspector (`Alt+P`) + centered **QuickLook (`Space`)** modal with local image decoding, fast syntax-highlighted code viewer, native Markdown rendering, folder summaries, and high-res Windows Shell thumbnails for videos/PDFs/executables.
- **Customizable Sidebar:** Compact drive bars, Quick Access folders (right-click to remove any default folder; restore anytime in Settings), Pinned folders, and automatic WSL distribution discovery.
- **Safe Windows Default File Manager Integration:** Per-user `HKCU\Software\Classes` registration (no UAC), automatic backup and one-click restore (`Settings` or `texplorer --restore-explorer`), Single-Instance Named Pipe routing, optional `Win+E` routing, and automatic `explorer.exe` passthrough for system CLSIDs/Control Panel.
- **Built-in Auto-Updater:** Checks GitHub Releases automatically, verifies `SHA256SUMS.txt` and Windows PE headers, and self-updates seamlessly.

## Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `Ctrl+T` / `Ctrl+W` | New / Close tab |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / Previous tab |
| `Ctrl+\` or `F3` | Toggle Dual-Pane Split |
| `Tab` | Switch active split pane |
| `Ctrl+L` / `Alt+D` | Edit address bar |
| `Alt+Left` / `Alt+Right` / `Alt+Up` | Back / Forward / Up |
| `Ctrl+F` | Focus inline folder filter |
| `Space` | Open / Close QuickLook preview (`↑`/`↓` to step files) |
| `Ctrl+B` / `Alt+P` | Toggle Sidebar / Inspector |
| `Ctrl+H` | Toggle hidden files |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copy / Cut / Paste (`CF_HDROP`) |
| `Ctrl+Z` / `Ctrl+Y` | Undo / Redo file operation |
| `Ctrl+Shift+N` / `F2` | New folder / Rename |
| `Del` / `Shift+Del` | Move to Recycle Bin / Delete permanently |
| `Shift+Right Click` | Open native Windows Shell context menu |
| `Ctrl+,` | Open Settings |

## Building from Source

```powershell
cargo test --workspace --locked
cargo build --release -p texplorer-app --locked
```

## License

MIT — see [LICENSE](LICENSE).
