/// A Tailscale peer machine
#[derive(Clone, Debug)]
pub struct TailscalePeer {
    pub host: String,
    pub ip: String,
    pub os: String,
    pub online: bool,
}

/// Run `tailscale status` and parse peers. Returns empty vec if Tailscale is not installed.
pub fn list_peers() -> Vec<TailscalePeer> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let output = std::process::Command::new("tailscale")
        .arg("status")
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    parse(&text)
}

fn parse(text: &str) -> Vec<TailscalePeer> {
    let mut peers = Vec::new();
    let mut first = true;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        // Skip the first line (self)
        if first {
            first = false;
            continue;
        }
        // Format: IP  HOST  USER  OS  STATUS...
        let ip = parts[0].to_string();
        let host = parts[1].to_string();
        let os = parts[3].to_string();
        let status = parts[4..].join(" ").to_lowercase();
        let online = !status.contains("offline") && !status.contains("-");
        peers.push(TailscalePeer { host, ip, os, online });
    }
    peers
}
