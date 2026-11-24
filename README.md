<p align="center"><img src="assets/icon.svg" width="112" alt="DirScope icon"></p>

# DirScope

A fast disk usage explorer for Windows and Linux, written in Rust with
[gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) (the UI framework behind the Zed editor).
Scan a drive or folder, see at a glance what is taking up space, and delete it.

## Features

- **Fast parallel scan** using every CPU core, with live file, folder and byte counters (Esc cancels)
- **Size tree** sorted largest first, with a share-of-parent bar, percentage and file count on every row
- **Delete** to the Recycle Bin / trash or permanently. Totals update instantly, with no rescan needed
- **Show in Explorer** / file manager for the selected item
- **Windows:** drive buttons, plus an offer to restart as administrator so system folders can be read fully
- Symlinks and junctions are skipped, so nothing is counted twice and scans never loop

## Download

Grab the latest build from the [Releases](https://github.com/rft0/dirscope/releases) page:

| Platform | File |
| --- | --- |
| Windows 10/11 (x64) | `dirscope-windows-x86_64.zip` |
| Linux (x64, glibc 2.35+) | `dirscope-linux-x86_64.tar.gz` |

On Linux the app needs a Vulkan-capable graphics driver (Mesa is fine) and either Wayland or X11.
To add it to your app launcher:

```
tar -xzf dirscope-linux-x86_64.tar.gz && cd dirscope
install -Dm755 dirscope ~/.local/bin/dirscope
install -Dm644 dirscope.png ~/.local/share/icons/hicolor/512x512/apps/dirscope.png
install -Dm644 dirscope.desktop ~/.local/share/applications/dirscope.desktop
```

You can also pass a folder to scan on the command line:

```
dirscope C:\Users
dirscope ~/projects
```

## Keyboard

| Key | Action |
| --- | --- |
| ↑ / ↓, PgUp / PgDn, Home / End | Move selection |
| → / ← | Expand / collapse (or jump to child / parent) |
| Enter, Space, double-click | Toggle folder |
| Del | Delete selected item (asks first) |
| F5 | Rescan |
| Ctrl+O | Open folder |
| Esc | Cancel scan |

## Building from source

Requires a recent stable Rust toolchain.

```
cargo run --release
```

On Debian/Ubuntu, install the system libraries first:

```
sudo apt install pkg-config libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libfontconfig1-dev libfreetype6-dev libx11-xcb-dev libxcb1-dev libvulkan-dev libssl-dev libzstd-dev
```

## License

[MIT](LICENSE)
