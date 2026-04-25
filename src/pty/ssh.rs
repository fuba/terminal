use super::PtyBackend;
use async_trait::async_trait;
use russh::client;
use russh::ChannelMsg;
use serde::{Deserialize, Serialize};
use std::io;
use std::sync::Arc;
use tokio::sync::mpsc as tmpsc;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_USER};

const WM_PTY_OUTPUT: u32 = WM_USER + 1;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SshProfile {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    /// "password" or "key"
    #[serde(default = "default_auth")]
    pub auth: String,
    /// Password if auth = "password" (plaintext — consider external secret storage)
    pub password: Option<String>,
    /// Path to private key if auth = "key"
    pub key_path: Option<String>,
}

fn default_port() -> u16 { 22 }
fn default_auth() -> String { "key".into() }

#[derive(Deserialize, Serialize, Clone, Debug)]
pub enum SshAuth {
    Password(String),
    Key(String),
}

enum SshCmd {
    Write(Vec<u8>),
    Resize(u16, u16),
    Close,
}

pub struct SshPty {
    cmd_tx: tmpsc::UnboundedSender<SshCmd>,
}

impl SshPty {
    /// Spawn an SSH session. Bytes received from the server are sent via `output_tx`
    /// and a WM_PTY_OUTPUT message is posted to `hwnd`.
    pub fn spawn(
        profile: SshProfile,
        cols: u16,
        rows: u16,
        output_tx: std::sync::mpsc::Sender<Vec<u8>>,
        hwnd: HWND,
    ) -> io::Result<Self> {
        let (cmd_tx, cmd_rx) = tmpsc::unbounded_channel();
        let hwnd_raw = hwnd.0 as isize;

        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = output_tx.send(
                        format!("\r\n[SSH] runtime error: {}\r\n", e).into_bytes(),
                    );
                    unsafe {
                        let _ = PostMessageW(
                            HWND(hwnd_raw as *mut _),
                            WM_PTY_OUTPUT,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                    return;
                }
            };
            let result = rt.block_on(ssh_main(profile, cols, rows, cmd_rx, &output_tx, hwnd_raw));
            if let Err(e) = result {
                let msg = format!("\r\n[SSH] {}\r\n", e);
                let _ = output_tx.send(msg.into_bytes());
                unsafe {
                    let _ = PostMessageW(
                        HWND(hwnd_raw as *mut _),
                        WM_PTY_OUTPUT,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        });

        Ok(SshPty { cmd_tx })
    }
}

impl PtyBackend for SshPty {
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.cmd_tx
            .send(SshCmd::Write(data.to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "ssh session closed"))
    }
    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let _ = self.cmd_tx.send(SshCmd::Resize(cols, rows));
        Ok(())
    }
}

impl Drop for SshPty {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(SshCmd::Close);
    }
}

struct SshHandler;

#[async_trait]
impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // TOFU: accept any key (TODO: implement known_hosts)
        Ok(true)
    }
}

async fn ssh_main(
    profile: SshProfile,
    cols: u16,
    rows: u16,
    mut cmd_rx: tmpsc::UnboundedReceiver<SshCmd>,
    output_tx: &std::sync::mpsc::Sender<Vec<u8>>,
    hwnd_raw: isize,
) -> Result<(), String> {
    let _ = output_tx.send(
        format!("\r\nConnecting to {}@{}:{}...\r\n", profile.user, profile.host, profile.port)
            .into_bytes(),
    );
    post_msg(hwnd_raw);

    let config = Arc::new(client::Config::default());
    let mut session = client::connect(config, (profile.host.as_str(), profile.port), SshHandler)
        .await
        .map_err(|e| format!("connect failed: {}", e))?;

    // Authenticate
    let authed = if profile.auth == "password" {
        let password = profile.password.as_deref().unwrap_or("");
        session
            .authenticate_password(&profile.user, password)
            .await
            .map_err(|e| format!("auth failed: {}", e))?
    } else {
        let path = profile.key_path.ok_or_else(|| "key_path required".to_string())?;
        let path = expand_path(&path);
        let key = russh_keys::load_secret_key(&path, None)
            .map_err(|e| format!("key load failed: {}", e))?;
        session
            .authenticate_publickey(&profile.user, Arc::new(key))
            .await
            .map_err(|e| format!("auth failed: {}", e))?
    };

    if !authed {
        return Err("authentication rejected".into());
    }

    let _ = output_tx.send(b"Connected.\r\n".to_vec());
    post_msg(hwnd_raw);

    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("channel open failed: {}", e))?;

    channel
        .request_pty(false, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await
        .map_err(|e| format!("pty request failed: {}", e))?;

    channel
        .request_shell(true)
        .await
        .map_err(|e| format!("shell request failed: {}", e))?;

    loop {
        tokio::select! {
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        if output_tx.send(data.to_vec()).is_err() { break; }
                        post_msg(hwnd_raw);
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if output_tx.send(data.to_vec()).is_err() { break; }
                        post_msg(hwnd_raw);
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SshCmd::Write(data)) => {
                        if channel.data(&data[..]).await.is_err() { break; }
                    }
                    Some(SshCmd::Resize(c, r)) => {
                        let _ = channel.window_change(c as u32, r as u32, 0, 0).await;
                    }
                    Some(SshCmd::Close) | None => break,
                }
            }
        }
    }

    let _ = output_tx.send(b"\r\n[SSH] Session ended.\r\n".to_vec());
    post_msg(hwnd_raw);
    Ok(())
}

fn post_msg(hwnd_raw: isize) {
    unsafe {
        let _ = PostMessageW(
            HWND(hwnd_raw as *mut _),
            WM_PTY_OUTPUT,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

fn expand_path(p: &str) -> std::path::PathBuf {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(p)
}
