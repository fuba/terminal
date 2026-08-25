use super::PtyBackend;
use async_trait::async_trait;
use russh::client;
use russh::ChannelMsg;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
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

enum SshCmd {
    Write(Vec<u8>),
    Resize(u16, u16),
    PasteImage {
        png: Vec<u8>,
        bracketed_paste: bool,
    },
    PasteFiles {
        paths: Vec<PathBuf>,
        bracketed_paste: bool,
    },
    Close,
}

type UploadFuture<'a> =
    Pin<Box<dyn Future<Output = (Result<Vec<String>, String>, bool)> + 'a>>;

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

    fn supports_clipboard_upload(&self) -> bool {
        true
    }

    fn paste_image(&mut self, png: Vec<u8>, bracketed_paste: bool) -> io::Result<bool> {
        self.cmd_tx
            .send(SshCmd::PasteImage {
                png,
                bracketed_paste,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "ssh session closed"))?;
        Ok(true)
    }

    fn paste_files(&mut self, paths: Vec<PathBuf>, bracketed_paste: bool) -> io::Result<bool> {
        self.cmd_tx
            .send(SshCmd::PasteFiles {
                paths,
                bracketed_paste,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "ssh session closed"))?;
        Ok(true)
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

    let mut upload: Option<UploadFuture<'_>> = None;
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
                    Some(SshCmd::PasteImage { png, bracketed_paste }) => {
                        if upload.is_none() {
                            let session = &session;
                            upload = Some(Box::pin(async move {
                                (
                                    upload_clipboard_image(session, &png)
                                        .await
                                        .map(|path| vec![path]),
                                    bracketed_paste,
                                )
                            }));
                        } else {
                            report_upload_error(output_tx, hwnd_raw, "another upload is still running");
                        }
                    }
                    Some(SshCmd::PasteFiles { paths, bracketed_paste }) => {
                        if upload.is_none() {
                            let session = &session;
                            upload = Some(Box::pin(async move {
                                (upload_local_files(session, paths).await, bracketed_paste)
                            }));
                        } else {
                            report_upload_error(output_tx, hwnd_raw, "another upload is still running");
                        }
                    }
                    Some(SshCmd::Close) | None => break,
                }
            }
            completed = async {
                match upload.as_mut() {
                    Some(future) => future.await,
                    None => std::future::pending().await,
                }
            } => {
                upload = None;
                match completed {
                    (Ok(paths), bracketed_paste) => {
                        let pasted = paths.join(" ");
                        if bracketed_paste {
                            let _ = channel.data(&b"\x1b[200~"[..]).await;
                        }
                        let _ = channel.data(pasted.as_bytes()).await;
                        if bracketed_paste {
                            let _ = channel.data(&b"\x1b[201~"[..]).await;
                        }
                    }
                    (Err(e), _) => report_upload_error(output_tx, hwnd_raw, &e),
                }
            }
        }
    }

    let _ = output_tx.send(b"\r\n[SSH] Session ended.\r\n".to_vec());
    post_msg(hwnd_raw);
    Ok(())
}

fn report_upload_error(
    output_tx: &std::sync::mpsc::Sender<Vec<u8>>,
    hwnd_raw: isize,
    detail: &str,
) {
    let _ = output_tx.send(
        format!("\r\n[SSH] clipboard upload failed: {detail}\r\n").into_bytes(),
    );
    post_msg(hwnd_raw);
}

fn clipboard_image_name(png: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    png.hash(&mut hasher);
    format!("terminal-paste-{:016x}.png", hasher.finish())
}

async fn upload_clipboard_image(
    session: &client::Handle<SshHandler>,
    png: &[u8],
) -> Result<String, String> {
    let filename = clipboard_image_name(png);
    let remote_path = format!("/tmp/{filename}");
    let mut scp = session
        .channel_open_session()
        .await
        .map_err(|e| format!("SCP channel open failed: {e}"))?;

    scp.exec(true, b"scp -t /tmp".to_vec())
        .await
        .map_err(|e| format!("SCP start failed: {e}"))?;
    read_scp_ack(&mut scp).await?;

    let header = format!("C0600 {} {filename}\n", png.len());
    scp.data(header.as_bytes())
        .await
        .map_err(|e| format!("SCP header failed: {e}"))?;
    read_scp_ack(&mut scp).await?;

    scp.data(png)
        .await
        .map_err(|e| format!("SCP data failed: {e}"))?;
    scp.data(&[0u8][..])
        .await
        .map_err(|e| format!("SCP completion failed: {e}"))?;
    read_scp_ack(&mut scp).await?;
    let _ = scp.eof().await;
    let _ = scp.close().await;

    Ok(remote_path)
}

async fn upload_local_files(
    session: &client::Handle<SshHandler>,
    paths: Vec<PathBuf>,
) -> Result<Vec<String>, String> {
    let mut scp = session
        .channel_open_session()
        .await
        .map_err(|e| format!("SCP channel open failed: {e}"))?;
    scp.exec(true, b"scp -t /tmp".to_vec())
        .await
        .map_err(|e| format!("SCP start failed: {e}"))?;
    read_scp_ack(&mut scp).await?;

    let mut remote_paths = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if !metadata.is_file() {
            continue;
        }
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        let filename = clipboard_file_name(&path, &metadata);
        let header = format!("C0600 {} {filename}\n", metadata.len());
        scp.data(header.as_bytes())
            .await
            .map_err(|e| format!("SCP header failed: {e}"))?;
        read_scp_ack(&mut scp).await?;
        scp.data(file)
            .await
            .map_err(|e| format!("SCP data failed for {}: {e}", path.display()))?;
        scp.data(&[0u8][..])
            .await
            .map_err(|e| format!("SCP completion failed: {e}"))?;
        read_scp_ack(&mut scp).await?;
        remote_paths.push(format!("/tmp/{filename}"));
    }

    let _ = scp.eof().await;
    let _ = scp.close().await;
    if remote_paths.is_empty() {
        Err("the clipboard did not contain any regular files".into())
    } else {
        Ok(remote_paths)
    }
}

fn clipboard_file_name(path: &Path, metadata: &std::fs::Metadata) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    if let Ok(modified) = metadata.modified() {
        modified.hash(&mut hasher);
    }

    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let mut safe_name = sanitize_filename_part(stem, 60);
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        let extension = sanitize_filename_part(extension, 16);
        if !extension.is_empty() {
            safe_name.push('.');
            safe_name.push_str(&extension);
        }
    }
    if safe_name.is_empty() {
        safe_name.push_str("file");
    }
    format!("terminal-paste-{:016x}-{safe_name}", hasher.finish())
}

fn sanitize_filename_part(value: &str, max_len: usize) -> String {
    let mut safe = String::with_capacity(value.len().min(max_len));
    for ch in value.chars().take(max_len) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            safe.push(ch);
        } else {
            safe.push('_');
        }
    }
    safe
}

async fn read_scp_ack(channel: &mut russh::Channel<client::Msg>) -> Result<(), String> {
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { data }) if !data.is_empty() => match data[0] {
                0 => return Ok(()),
                1 | 2 => {
                    let message = String::from_utf8_lossy(&data[1..]);
                    return Err(message.trim().to_string());
                }
                code => return Err(format!("unexpected SCP response: {code}")),
            },
            Some(ChannelMsg::ExtendedData { data, .. }) if !data.is_empty() => {
                return Err(String::from_utf8_lossy(&data).trim().to_string());
            }
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                return Err("SCP channel closed before acknowledgement".into());
            }
            _ => {}
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{clipboard_file_name, clipboard_image_name};

    #[test]
    fn clipboard_image_name_is_stable_and_safe() {
        let name = clipboard_image_name(b"png data");
        assert_eq!(name, clipboard_image_name(b"png data"));
        assert!(name.starts_with("terminal-paste-"));
        assert!(name.ends_with(".png"));
        assert!(!name.contains('/'));
    }

    #[test]
    fn clipboard_file_name_preserves_a_safe_extension() {
        let path = std::env::temp_dir().join("movie sample.mp4");
        std::fs::write(&path, b"video").expect("create temporary file");
        let metadata = std::fs::metadata(&path).expect("read temporary metadata");
        let name = clipboard_file_name(&path, &metadata);
        let _ = std::fs::remove_file(path);

        assert!(name.starts_with("terminal-paste-"));
        assert!(name.ends_with("-movie_sample.mp4"));
        assert!(!name.contains(' '));
    }
}
