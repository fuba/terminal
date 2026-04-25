mod app;
mod config;
mod crash;
mod image;
mod keys;
mod log;
mod pty;
mod render;
mod terminal;
mod url;

fn main() {
    crash::install();
    if let Err(e) = app::run() {
        eprintln!("Fatal error: {e}");
        std::process::exit(1);
    }
}
