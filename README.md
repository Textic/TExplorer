# TExplorer

Fast, minimalist flat-dark Windows file explorer built with **Rust** and **GPUI (`gpui-kit`)**.

## Features (v0.1.0)

- **GPU-Accelerated Obsidian Dark UI:** Strict flat dark palette (`#09090B` / `#111113` / `#18181B` / `#27272A`) with `1px` panel cuts, zero shadows, selectable accents (*Emerald*, *Violet*, *Monochrome*), and a custom `38px` frameless titlebar.
- **Tabs & Dual-Pane Split View:** Optional top tab bar (`Settings → Top Tab Bar`) and independent per-tab Dual-Pane split (`Ctrl+\` or `F3`).
- **Virtualized High-Speed Views:** Fast Win32 directory enumeration (`FindFirstFileExW`) with **Details**, **Grid**, **Compact**, and **Home** views, natural sorting, multi-selection, live directory watcher (`ReadDirectoryChangesW`), inline filter (`Ctrl+F`), and type-ahead jump.
- **Customizable Sidebar:** Compact drive bars, Quick Access folders (right-click to remove any default folder; restore anytime in Settings), Pinned folders, and automatic WSL distribution discovery.
- **Safe Windows Default File Manager Integration:** Per-user `HKCU\Software\Classes` registration (no UAC), automatic backup and one-click restore (`Settings` or `texplorer --restore-explorer`), Single-Instance Named Pipe tab routing, optional `Win+E` routing, and automatic `explorer.exe` passthrough for system CLSIDs/Control Panel.
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
| `Ctrl+B` / `Alt+P` | Toggle Sidebar / Inspector |
| `Ctrl+H` | Toggle hidden files |
| `Ctrl+Shift+N` / `F2` | New folder / Rename |
| `Del` / `Shift+Del` | Move to Recycle Bin / Delete permanently |
| `Ctrl+,` | Open Settings |

## Building from Source

```powershell
cargo test --workspace
cargo build --release -p texplorer-app
```

## License

MIT — see [LICENSE](LICENSE).
