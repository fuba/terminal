# Repository Guidelines

## Project Structure & Module Organization

This repository contains a Windows terminal emulator written in Rust. `src/main.rs` is the executable entry point. Feature modules live under `src/`: `app/` handles window behavior and integrations, `terminal/` owns parsing and grid state, `render/` provides GPU rendering, `pty/` contains ConPTY and SSH backends, and `image/` implements terminal image protocols. Shared configuration, key handling, logging, performance, crash, and URL logic are in their corresponding modules. Windows manifests, icons, and resource scripts live in `resources/`. Treat `target/` as generated build output.

## Build, Test, and Development Commands

- `cargo build` compiles a debug build for local development.
- `cargo run` builds and launches the terminal with a development console.
- `cargo build --release --target x86_64-pc-windows-msvc` reproduces the CI/release build.
- `cargo test` runs all Rust tests; add tests as behavior becomes independently testable.
- `cargo fmt --all -- --check` verifies standard Rust formatting.
- `cargo clippy --all-targets --all-features -- -D warnings` catches common correctness and style issues.
- `cargo install --path .` installs the executable from the working tree.

Development requires Windows, the stable Rust toolchain, and the MSVC target. Copy settings from `config.example.toml`; runtime configuration belongs at `%APPDATA%\terminal\config.toml` and must not be committed.

## Updating the User's Installed Terminal

After every executable change, build and validate the release, then update the user's normal launch location before reporting completion. Leaving the new executable only under `target/` is not sufficient. This update is explicitly authorized by the user and does not require asking again.

Resolve the launch path with `Get-Command terminal` and account for junctions. The current installation resolves to `C:\Users\ec\scoop\persist\rustup\.cargo\bin\terminal.exe`. Verify the installed executable's hash matches the release build. Preserve a recoverable backup when replacing it; if it is running, use a staged replacement without terminating existing terminal sessions. Tell the user when a restart is needed. If replacement fails, clearly report that the normal launch location has not been updated.

## Coding Style & Naming Conventions

Use `rustfmt` defaults (four-space indentation). Follow Rust conventions: `snake_case` for functions, variables, and modules; `PascalCase` for types and traits; `SCREAMING_SNAKE_CASE` for constants. Keep Windows `unsafe` blocks narrow and document the safety assumptions around handles, pointers, and COM lifetimes. Prefer focused modules over expanding `main.rs`.

## Testing Guidelines

There is currently no committed automated test suite or coverage threshold. Place unit tests beside implementation code in `#[cfg(test)] mod tests`; use descriptive names such as `parses_split_sgr_sequence`. Prioritize parser, grid, configuration, SSH-config, and image-decoder edge cases. Run formatting, Clippy, tests, and a release build before submitting changes. Manually verify UI/rendering changes on Windows and record the scenario tested.

## Commit & Pull Request Guidelines

Recent commits use concise, imperative, area-prefixed subjects, for example `Render: pixel-snap box glyph edges` and `Fix: first char eaten after non-char keybindings`. Keep each commit focused. Pull requests should explain the user-visible effect, implementation approach, and validation performed; link relevant issues and include screenshots or short recordings for rendering, settings, or window-behavior changes. Ensure Windows CI passes and do not commit generated artifacts from `target/`.
