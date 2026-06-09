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
├── perf.rs              # frame-timing profiler (slow frames → perf.log)
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

## Performance

### Frame profiler

Every `WM_PAINT` and `WM_PTY_OUTPUT` handler is timed with `Instant`-based
phase markers. Frames above the threshold (default 16ms) are appended to
`%APPDATA%\terminal\perf.log` with a per-phase breakdown; faster frames only
bump in-memory counters. Every 50 slow frames a `[summary]` line records the
running slow-frame percentage.

```
2026-05-19 02:50:43.366 [pty] total=16.30ms process=0.00ms render=16.30ms tail=0.00ms
2026-05-19 02:50:38.611 [summary] frames=41247 slow=3300 (8.00%)
```

- `process` covers PTY drain + VT parse + response write-back
- `render` covers the entire frame paint
- `tail` is anything after the last marker

Override the threshold with `TERMINAL_SLOW_FRAME_MS=8` for a noisier sweep
while investigating. The log is append-only across runs; rotate it manually.

### Renderer batching and caching

The grid loop in `src/render/mod.rs` runs three passes per row to keep
Direct2D draw calls and brush allocations down — the dominant cost for a
heavily decorated full-screen TUI (e.g. claude-code under tmux).

1. **Background pass** — consecutive cells with the same background color
   coalesce into a single `FillRectangle`.
2. **Text pass** — consecutive width-1 cells with the same `(fg, bold, dim)`
   coalesce into a single `DrawText` over a multi-cell rect. Width-2 (CJK)
   cells flush the run and draw standalone to avoid glyph-advance drift in
   mixed-width runs. Runs containing only spaces are dropped before emit.
3. **Underline pass** — consecutive underlined cells with the same color
   coalesce into a single `DrawLine`.

`BrushCache` (`HashMap<u32, ID2D1SolidColorBrush>`) caches solid-color
brushes across frames keyed on quantized RGBA. Caches are tied to the render
target and cleared in `ensure_target` whenever Direct2D signals
`D2DERR_RECREATE_TARGET` (device loss). A 256-entry ceiling guards against
unbounded growth from 24-bit colorspaces; eviction is a simple flush.

`BitmapCache` keeps decoded image bitmaps by `data.as_ptr() as usize`,
skipping `CreateBitmap` on every frame where an image is on screen. Stored
images are append-only so the pointer is stable for the image's lifetime.

The measured effect on a claude-code-in-tmux workload was a slow-frame rate
of **16% → 8%** with 30–47ms spikes eliminated entirely; remaining slow
frames cluster just above the 16ms threshold.

### Diagnosing future hitches

When `perf.log` shows new slow frames, read the phase breakdown first:

- `process` dominant → look at `terminal::parser` / `terminal::handler` or
  the PTY drain loop in `app::mod::WM_PTY_OUTPUT`
- `render` dominant → look at the per-row passes in `render::mod::render`,
  or at any draw added below the grid loop (cursor / overlay / scrollbar)
- both small but `total` large → something outside the marked phases (the
  `tail` segment) — add a `mark()` call there to narrow it down

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
