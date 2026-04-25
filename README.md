# Terminal

A native Windows terminal emulator written from scratch in Rust. Uses the Win32
API directly with Direct2D/DirectWrite for rendering and ConPTY for local
shells.

Built primarily as a learning exercise and personal daily-driver. Not intended
to compete with Windows Terminal or WezTerm.

## Features

- **Rendering**: Direct2D + DirectWrite, ClearType, CJK font fallback,
  per-monitor DPI awareness
- **Tabs**: Multiple tabs with native Windows popup menu for new-tab options
- **Local shells**: ConPTY-backed PowerShell, pwsh, cmd, WSL, Git Bash
  (auto-detected)
- **SSH**: Pure-Rust SSH client (`russh`) with `~/.ssh/config` auto-loading
- **Tailscale**: Auto-discovers Tailscale peers and lists them in the menu
- **Bookmarks & favorites**: Pin frequently-used shells/SSH targets
- **VT compatibility**: 256-color/truecolor, scrollback, alt screen, scroll
  regions, mouse tracking (X10 + SGR), bracketed paste, DECCKM, OSC titles
- **Image protocols**: Sixel decoder, iTerm2 inline images
  (note: ConPTY strips DCS, so Sixel only works through SSH)
- **Clipboard**: Win32 clipboard + OSC 52
- **IME**: Inline Japanese input with composition shown at cursor; candidate
  popup follows the cursor
- **Global hotkey**: Configurable system-wide show/hide (default Alt+Shift+V)
- **Window memory**: Last position remembered per display; falls back safely
  when a display is disconnected
- **Dock to height**: F11 expands the window to full work-area height,
  re-snaps when displays change
- **Settings GUI**: Live-applies colors, opacity, columns/rows; native color
  picker
- **Session logging**: Per-tab toggle (Ctrl+L); strips escape sequences
- **Crash log**: Rust panics and Win32 unhandled exceptions captured to
  `%APPDATA%\terminal\crash.log`

## Install

Requires Rust toolchain with the MSVC target and Visual Studio Build Tools
(C++ workload + Windows SDK).

```sh
cargo install --path .
```

This installs `terminal.exe` to `%USERPROFILE%\.cargo\bin` (already on PATH if
you used rustup). Then just run:

```sh
terminal
```

## Configuration

Config file: `%APPDATA%\terminal\config.toml`

The settings GUI (Ctrl+,) writes to this file. Manual editing is also
supported. See `config.example.toml` for all available options.

```toml
shell = "powershell.exe"
font_family = "Consolas"
font_size = 16.0
columns = 120
rows = 30
opacity = 100
fg_color = "#CCCCCC"
bg_color = "#0C0C0C"
scrollback_limit = 10000

[hotkey]
enabled = true
modifiers = "alt,shift"  # ctrl, alt, shift, win (comma-separated)
key = "v"                # a-z, 0-9, grave, space, f1-f12, tab

# Optional: bookmarks shown in the [+ ▾] menu
[[bookmarks]]
name = "WSL Ubuntu"
shell = "wsl.exe -d Ubuntu"

[[bookmarks]]
name = "Production"
ssh = "prod-server"     # references an ssh_profiles entry by name

# Optional: SSH profiles (also reads ~/.ssh/config automatically)
[[ssh_profiles]]
name = "myserver"
host = "example.com"
port = 22
user = "myuser"
auth = "key"
key_path = "~/.ssh/id_ed25519"
```

## Keyboard Shortcuts

| Key                | Action                            |
|--------------------|-----------------------------------|
| Ctrl+T             | New tab (default shell)           |
| Ctrl+W             | Close current tab                 |
| Ctrl+→ / Ctrl+←   | Next / previous tab               |
| Ctrl+1 ‥ Ctrl+9    | Select tab N                      |
| Ctrl+C             | Copy if selection, else send ^C   |
| Ctrl+V             | Paste                             |
| Ctrl+↑ / Ctrl+↓   | Scrollback page up / down         |
| Ctrl+Home / End    | Scrollback top / bottom           |
| Ctrl+,             | Settings                          |
| Ctrl+L             | Toggle session log                |
| F11                | Toggle dock-to-full-height        |
| Alt+Shift+V        | Global hotkey: show/hide window   |

## Mouse

- **Drag** to select; selection is auto-cleared on type
- **Right-click** in terminal area: paste
- **Ctrl+Click** on a URL: open in default browser (with hover underline)
- **Click [+ ▾]** in tab bar: new-tab menu (shells / bookmarks / SSH /
  Tailscale)
- **Right-click on menu item**: toggle favorite (★ items pinned to top)
- **Click [⚙]**: settings
- **Click [×]** on a tab: close it

## Architecture

```
src/
├── main.rs              # entry, install crash handler
├── crash.rs             # Rust panic + Win32 exception → log file
├── app/
│   ├── mod.rs           # window, message loop, tab management
│   ├── settings.rs      # settings GUI (Win32 controls)
│   ├── ssh_picker.rs    # SSH profile picker dialog
│   ├── tailscale.rs     # `tailscale status` parser
│   └── window_state.rs  # per-monitor window position memory
├── render/mod.rs        # Direct2D/DirectWrite renderer
├── terminal/
│   ├── parser.rs        # VT state machine (CSI/OSC/DCS/ESC/UTF-8)
│   ├── handler.rs       # interprets actions → grid mutations
│   ├── grid.rs          # cell grid + scrollback + alt screen
│   ├── cell.rs          # Cell, Color, attributes
│   └── selection.rs     # text selection
├── pty/
│   ├── conpty.rs        # local PTY (ConPTY)
│   ├── ssh.rs           # SSH backend (russh + tokio)
│   └── ssh_config.rs    # ~/.ssh/config parser
├── image/
│   ├── sixel.rs         # Sixel → RGBA decoder
│   ├── iterm2.rs        # iTerm2 inline image (OSC 1337)
│   └── osc52.rs         # OSC 52 clipboard
├── config/mod.rs        # TOML config (load/save)
├── keys/mod.rs          # keybinding engine
├── url/mod.rs           # URL detection (regex + ShellExecute)
└── log/mod.rs           # session logger
```

### Why these choices

| Choice                    | Reason                                                           |
|---------------------------|------------------------------------------------------------------|
| Raw Win32 (no framework)  | Terminal is one custom render surface; widgets unnecessary       |
| Direct2D / DirectWrite    | Built-in ClearType, CJK fallback, complex script shaping         |
| `russh` (pure Rust)       | No C dependency, async, modern algorithms                        |
| Internal UTF-8            | Convert at boundaries only (UTF-16 for Win32 / UTF-8 elsewhere)  |
| Trait-based PTY backend   | Same Tab abstraction works for ConPTY and SSH                    |

## Known Limitations

- **Sixel via ConPTY**: ConPTY strips DCS sequences, so Sixel images sent from
  local programs do not render. Works when received over SSH.
- **SSH host key verification**: Currently TOFU (trust on first use); no
  `known_hosts` checking yet.
- **No xterm-style font ligatures**: DirectWrite handles ligatures, but the
  per-cell rendering breaks them across cell boundaries.
- **Single window**: No "new window" command; uses one window with multiple
  tabs.

## License

MIT
