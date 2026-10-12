use super::PtyBackend;
use async_trait::async_trait;
use russh::client;
use russh::ChannelMsg;
use serde::{Deserialize, Serialize};
use std::collections::{hash_map::DefaultHasher, VecDeque};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc as tmpsc;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_USER};

const WM_PTY_OUTPUT: u32 = WM_USER + 1;
const SSH_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const SSH_KEEPALIVE_MAX: usize = 3;
const SSH_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const SSH_AUTH_TIMEOUT: Duration = Duration::from_secs(20);
const SSH_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);

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

fn default_port() -> u16 {
    22
}
fn default_auth() -> String {
    "key".into()
}

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

#[derive(Copy, Clone)]
struct TerminalSize {
    cols: u16,
    rows: u16,
}

enum SessionExit {
    Stopped,
    RemoteClosed,
}

struct SshFailure {
    message: String,
    retryable: bool,
    was_connected: bool,
}

impl SshFailure {
    fn transient(message: impl Into<String>, was_connected: bool) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            was_connected,
        }
    }

    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            was_connected: false,
        }
    }
}

enum RetryAction {
    Retry,
    Stop,
    Continue,
}

type UploadFuture<'a> = Pin<Box<dyn Future<Output = (Result<Vec<String>, String>, bool)> + 'a>>;

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
        output_pending: Arc<AtomicBool>,
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
                    let _ =
                        output_tx.send(format!("\r\n[SSH] runtime error: {}\r\n", e).into_bytes());
                    post_msg(hwnd_raw, &output_pending);
                    return;
                }
            };
            rt.block_on(ssh_supervisor(
                profile,
                cols,
                rows,
                cmd_rx,
                &output_tx,
                hwnd_raw,
                &output_pending,
            ));
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

async fn ssh_supervisor(
    profile: SshProfile,
    cols: u16,
    rows: u16,
    mut cmd_rx: tmpsc::UnboundedReceiver<SshCmd>,
    output_tx: &std::sync::mpsc::Sender<Vec<u8>>,
    hwnd_raw: isize,
    output_pending: &AtomicBool,
) {
    let mut size = TerminalSize { cols, rows };
    let mut deferred = VecDeque::new();
    let mut failed_attempts = 0u32;

    loop {
        match ssh_main(
            &profile,
            &mut size,
            &mut cmd_rx,
            &mut deferred,
            output_tx,
            hwnd_raw,
            output_pending,
        )
        .await
        {
            Ok(SessionExit::Stopped) => return,
            Ok(SessionExit::RemoteClosed) => {
                failed_attempts = 0;
                if !report_ssh_status(
                    output_tx,
                    hwnd_raw,
                    output_pending,
                    "\r\n[SSH] Remote shell exited. Press Enter to reconnect.\r\n",
                ) {
                    return;
                }
                if matches!(
                    wait_for_retry(&mut cmd_rx, &mut size, &mut deferred, None).await,
                    RetryAction::Stop
                ) {
                    return;
                }
            }
            Err(failure) => {
                if failure.was_connected {
                    failed_attempts = 0;
                }

                if failure.retryable {
                    failed_attempts = failed_attempts.saturating_add(1);
                    let delay = reconnect_delay(failed_attempts);
                    let status = format!(
                        "\r\n[SSH] Connection lost: {}\r\n[SSH] Reconnecting in {}s (attempt {}; press any key to retry now).\r\n",
                        failure.message,
                        delay.as_secs(),
                        failed_attempts,
                    );
                    if !report_ssh_status(output_tx, hwnd_raw, output_pending, &status) {
                        return;
                    }
                    if matches!(
                        wait_for_retry(&mut cmd_rx, &mut size, &mut deferred, Some(delay)).await,
                        RetryAction::Stop
                    ) {
                        return;
                    }
                } else {
                    failed_attempts = 0;
                    let status = format!(
                        "\r\n[SSH] {}\r\n[SSH] Automatic retry paused. Press Enter to retry.\r\n",
                        failure.message,
                    );
                    if !report_ssh_status(output_tx, hwnd_raw, output_pending, &status) {
                        return;
                    }
                    if matches!(
                        wait_for_retry(&mut cmd_rx, &mut size, &mut deferred, None).await,
                        RetryAction::Stop
                    ) {
                        return;
                    }
                }
            }
        }
    }
}

async fn wait_for_retry(
    cmd_rx: &mut tmpsc::UnboundedReceiver<SshCmd>,
    size: &mut TerminalSize,
    deferred: &mut VecDeque<SshCmd>,
    delay: Option<Duration>,
) -> RetryAction {
    if let Some(delay) = delay {
        let timer = tokio::time::sleep(delay);
        tokio::pin!(timer);
        loop {
            tokio::select! {
                () = &mut timer => return RetryAction::Retry,
                cmd = cmd_rx.recv() => {
                    match handle_disconnected_command(cmd, size, deferred) {
                        RetryAction::Continue => {}
                        action => return action,
                    }
                }
            }
        }
    }

    loop {
        match handle_disconnected_command(cmd_rx.recv().await, size, deferred) {
            RetryAction::Continue => {}
            action => return action,
        }
    }
}

fn handle_disconnected_command(
    cmd: Option<SshCmd>,
    size: &mut TerminalSize,
    deferred: &mut VecDeque<SshCmd>,
) -> RetryAction {
    match cmd {
        Some(SshCmd::Close) | None => RetryAction::Stop,
        Some(SshCmd::Resize(cols, rows)) => {
            *size = TerminalSize { cols, rows };
            RetryAction::Continue
        }
        Some(SshCmd::Write(data)) if data.iter().all(|b| matches!(b, b'\r' | b'\n')) => {
            RetryAction::Retry
        }
        Some(cmd) => {
            // Preserve input/paste that triggered an immediate reconnect and
            // deliver it after the remote shell is ready again.
            deferred.push_back(cmd);
            RetryAction::Retry
        }
    }
}

fn reconnect_delay(failed_attempts: u32) -> Duration {
    if failed_attempts >= 6 {
        return SSH_RECONNECT_MAX_DELAY;
    }
    Duration::from_secs(1u64 << failed_attempts.saturating_sub(1))
}

fn ssh_client_config() -> client::Config {
    client::Config {
        keepalive_interval: Some(SSH_KEEPALIVE_INTERVAL),
        keepalive_max: SSH_KEEPALIVE_MAX,
        ..Default::default()
    }
}

async fn next_command(
    cmd_rx: &mut tmpsc::UnboundedReceiver<SshCmd>,
    deferred: &mut VecDeque<SshCmd>,
) -> Option<SshCmd> {
    match deferred.pop_front() {
        Some(cmd) => Some(cmd),
        None => cmd_rx.recv().await,
    }
}

async fn ssh_main(
    profile: &SshProfile,
    size: &mut TerminalSize,
    cmd_rx: &mut tmpsc::UnboundedReceiver<SshCmd>,
    deferred: &mut VecDeque<SshCmd>,
    output_tx: &std::sync::mpsc::Sender<Vec<u8>>,
    hwnd_raw: isize,
    output_pending: &AtomicBool,
) -> Result<SessionExit, SshFailure> {
    if !report_ssh_status(
        output_tx,
        hwnd_raw,
        output_pending,
        &format!(
            "\r\nConnecting to {}@{}:{}...\r\n",
            profile.user, profile.host, profile.port
        ),
    ) {
        return Ok(SessionExit::Stopped);
    }

    let config = Arc::new(ssh_client_config());
    let connect = client::connect(config, (profile.host.as_str(), profile.port), SshHandler);
    let mut session = tokio::time::timeout(SSH_CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| SshFailure::transient("connection attempt timed out", false))?
        .map_err(|e| SshFailure::transient(format!("connect failed: {e}"), false))?;

    // Authentication/configuration failures require user action; pausing the
    // automatic loop avoids hammering a server with bad credentials.
    let auth = if profile.auth == "password" {
        let password = profile.password.as_deref().unwrap_or("");
        tokio::time::timeout(
            SSH_AUTH_TIMEOUT,
            session.authenticate_password(&profile.user, password),
        )
        .await
        .map_err(|_| SshFailure::transient("authentication timed out", false))?
        .map_err(|e| SshFailure::fatal(format!("authentication failed: {e}")))?
    } else {
        let path = profile
            .key_path
            .as_deref()
            .ok_or_else(|| SshFailure::fatal("key_path is required"))?;
        let path = expand_path(path);
        let key = russh_keys::load_secret_key(&path, None)
            .map_err(|e| SshFailure::fatal(format!("key load failed: {e}")))?;
        tokio::time::timeout(
            SSH_AUTH_TIMEOUT,
            session.authenticate_publickey(&profile.user, Arc::new(key)),
        )
        .await
        .map_err(|_| SshFailure::transient("authentication timed out", false))?
        .map_err(|e| SshFailure::fatal(format!("authentication failed: {e}")))?
    };

    if !auth {
        return Err(SshFailure::fatal("authentication rejected"));
    }

    let mut channel = tokio::time::timeout(SSH_CONNECT_TIMEOUT, session.channel_open_session())
        .await
        .map_err(|_| SshFailure::transient("channel open timed out", false))?
        .map_err(|e| SshFailure::transient(format!("channel open failed: {e}"), false))?;

    channel
        .request_pty(
            false,
            "xterm-256color",
            size.cols as u32,
            size.rows as u32,
            0,
            0,
            &[],
        )
        .await
        .map_err(|e| SshFailure::fatal(format!("PTY request failed: {e}")))?;

    channel
        .request_shell(true)
        .await
        .map_err(|e| SshFailure::fatal(format!("shell request failed: {e}")))?;

    if !report_ssh_status(output_tx, hwnd_raw, output_pending, "[SSH] Connected.\r\n") {
        return Ok(SessionExit::Stopped);
    }

    let mut upload: Option<UploadFuture<'_>> = None;
    let mut remote_reported_exit = false;
    loop {
        tokio::select! {
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        if output_tx.send(data.to_vec()).is_err() {
                            return Ok(SessionExit::Stopped);
                        }
                        post_msg(hwnd_raw, output_pending);
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if output_tx.send(data.to_vec()).is_err() {
                            return Ok(SessionExit::Stopped);
                        }
                        post_msg(hwnd_raw, output_pending);
                    }
                    Some(ChannelMsg::ExitStatus { .. }) | Some(ChannelMsg::ExitSignal { .. }) => {
                        remote_reported_exit = true;
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => {
                        if remote_reported_exit {
                            return Ok(SessionExit::RemoteClosed);
                        }
                        return Err(SshFailure::transient("channel closed", true));
                    }
                    None => {
                        return Err(SshFailure::transient("transport closed", true));
                    }
                    _ => {}
                }
            }
            cmd = next_command(cmd_rx, deferred) => {
                match cmd {
                    Some(SshCmd::Write(data)) => {
                        channel
                            .data(&data[..])
                            .await
                            .map_err(|e| SshFailure::transient(format!("write failed: {e}"), true))?;
                    }
                    Some(SshCmd::Resize(cols, rows)) => {
                        *size = TerminalSize { cols, rows };
                        let _ = channel.window_change(cols as u32, rows as u32, 0, 0).await;
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
                            report_upload_error(
                                output_tx,
                                hwnd_raw,
                                output_pending,
                                "another upload is still running",
                            );
                        }
                    }
                    Some(SshCmd::PasteFiles { paths, bracketed_paste }) => {
                        if upload.is_none() {
                            let session = &session;
                            upload = Some(Box::pin(async move {
                                (upload_local_files(session, paths).await, bracketed_paste)
                            }));
                        } else {
                            report_upload_error(
                                output_tx,
                                hwnd_raw,
                                output_pending,
                                "another upload is still running",
                            );
                        }
                    }
                    Some(SshCmd::Close) | None => return Ok(SessionExit::Stopped),
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
                    (Err(e), _) => {
                        report_upload_error(output_tx, hwnd_raw, output_pending, &e)
                    }
                }
            }
        }
    }
}

fn report_ssh_status(
    output_tx: &std::sync::mpsc::Sender<Vec<u8>>,
    hwnd_raw: isize,
    output_pending: &AtomicBool,
    status: &str,
) -> bool {
    if output_tx.send(status.as_bytes().to_vec()).is_err() {
        return false;
    }
    post_msg(hwnd_raw, output_pending);
    true
}

fn report_upload_error(
    output_tx: &std::sync::mpsc::Sender<Vec<u8>>,
    hwnd_raw: isize,
    output_pending: &AtomicBool,
    detail: &str,
) {
    let _ = output_tx.send(format!("\r\n[SSH] clipboard upload failed: {detail}\r\n").into_bytes());
    post_msg(hwnd_raw, output_pending);
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

fn post_msg(hwnd_raw: isize, output_pending: &AtomicBool) {
    if output_pending
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    unsafe {
        if PostMessageW(
            HWND(hwnd_raw as *mut _),
            WM_PTY_OUTPUT,
            WPARAM(0),
            LPARAM(0),
        )
        .is_err()
        {
            output_pending.store(false, Ordering::Release);
        }
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
    use super::{
        clipboard_file_name, clipboard_image_name, handle_disconnected_command, reconnect_delay,
        ssh_client_config, RetryAction, SshCmd, TerminalSize, SSH_KEEPALIVE_INTERVAL,
        SSH_KEEPALIVE_MAX, SSH_RECONNECT_MAX_DELAY,
    };
    use std::collections::VecDeque;
    use std::time::Duration;

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

    #[test]
    fn reconnect_delay_backs_off_and_caps_at_thirty_seconds() {
        let expected = [1, 2, 4, 8, 16, 30, 30];
        for (attempt, seconds) in (1..).zip(expected) {
            assert_eq!(reconnect_delay(attempt), Duration::from_secs(seconds));
        }
        assert_eq!(reconnect_delay(u32::MAX), SSH_RECONNECT_MAX_DELAY);
    }

    #[test]
    fn ssh_client_uses_keepalive_without_an_idle_disconnect() {
        let config = ssh_client_config();
        assert_eq!(config.keepalive_interval, Some(SSH_KEEPALIVE_INTERVAL));
        assert_eq!(config.keepalive_max, SSH_KEEPALIVE_MAX);
        assert_eq!(config.inactivity_timeout, None);
    }

    #[test]
    fn enter_retries_without_sending_enter_to_the_new_shell() {
        let mut size = TerminalSize { cols: 80, rows: 24 };
        let mut deferred = VecDeque::new();

        let action = handle_disconnected_command(
            Some(SshCmd::Write(b"\r".to_vec())),
            &mut size,
            &mut deferred,
        );

        assert!(matches!(action, RetryAction::Retry));
        assert!(deferred.is_empty());
    }

    #[test]
    fn typed_input_retries_and_is_delivered_after_reconnect() {
        let mut size = TerminalSize { cols: 80, rows: 24 };
        let mut deferred = VecDeque::new();

        let action = handle_disconnected_command(
            Some(SshCmd::Write(b"pwd".to_vec())),
            &mut size,
            &mut deferred,
        );

        assert!(matches!(action, RetryAction::Retry));
        match deferred.pop_front() {
            Some(SshCmd::Write(data)) => assert_eq!(data, b"pwd"),
            _ => panic!("typed input was not preserved"),
        }
    }

    #[test]
    fn resize_is_retained_without_interrupting_the_retry_timer() {
        let mut size = TerminalSize { cols: 80, rows: 24 };
        let mut deferred = VecDeque::new();

        let action =
            handle_disconnected_command(Some(SshCmd::Resize(132, 43)), &mut size, &mut deferred);

        assert!(matches!(action, RetryAction::Continue));
        assert_eq!(size.cols, 132);
        assert_eq!(size.rows, 43);
        assert!(deferred.is_empty());
    }
}
