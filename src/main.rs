mod app;
mod config;
mod keys;
mod pty;
mod render;
mod terminal;
mod url;

fn main() {
    if let Err(e) = app::run() {
        eprintln!("Fatal error: {e}");
        std::process::exit(1);
    }
}
