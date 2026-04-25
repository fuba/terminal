use super::ssh::SshProfile;
use std::path::PathBuf;

/// Parse ~/.ssh/config and return matching profiles.
pub fn load_profiles() -> Vec<SshProfile> {
    let path = match ssh_config_path() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    parse(&content)
}

fn ssh_config_path() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    Some(PathBuf::from(home).join(".ssh").join("config"))
}

fn parse(content: &str) -> Vec<SshProfile> {
    let mut profiles = Vec::new();
    let mut defaults = HostBlock::default();
    let mut current: Option<(String, HostBlock)> = None;

    let flush = |current: &mut Option<(String, HostBlock)>,
                 defaults: &HostBlock,
                 profiles: &mut Vec<SshProfile>| {
        if let Some((name, block)) = current.take() {
            // Skip wildcard-only hosts
            if name.contains('*') || name.contains('?') {
                return;
            }
            profiles.push(build_profile(name, block, defaults));
        }
    };

    for line in content.lines() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }

        let (key, value) = match split_kv(line) {
            Some(kv) => kv,
            None => continue,
        };

        match key.to_lowercase().as_str() {
            "host" => {
                flush(&mut current, &defaults, &mut profiles);
                // "Host *" or with wildcards → treat as defaults for subsequent specific hosts
                if value == "*" {
                    current = None;
                    // Collect into defaults on following directives
                    defaults = HostBlock::default();
                    current = Some(("*".into(), HostBlock::default()));
                } else {
                    // Multiple hosts on one line: pick first non-wildcard name
                    let name = value
                        .split_whitespace()
                        .find(|s| !s.contains('*') && !s.contains('?'))
                        .map(|s| s.to_string());
                    if let Some(n) = name {
                        current = Some((n, HostBlock::default()));
                    } else {
                        current = None;
                    }
                }
            }
            "hostname" => set_current(&mut current, &mut defaults, |b| b.host_name = Some(value.into())),
            "port" => {
                if let Ok(p) = value.parse::<u16>() {
                    set_current(&mut current, &mut defaults, |b| b.port = Some(p));
                }
            }
            "user" => set_current(&mut current, &mut defaults, |b| b.user = Some(value.into())),
            "identityfile" => {
                set_current(&mut current, &mut defaults, |b| b.identity_file = Some(value.into()))
            }
            _ => {}
        }
    }

    flush(&mut current, &defaults, &mut profiles);

    profiles
}

fn set_current<F: FnOnce(&mut HostBlock)>(
    current: &mut Option<(String, HostBlock)>,
    defaults: &mut HostBlock,
    setter: F,
) {
    if let Some((name, block)) = current.as_mut() {
        if name == "*" {
            setter(defaults);
        } else {
            setter(block);
        }
    } else {
        setter(defaults);
    }
}

fn build_profile(name: String, block: HostBlock, defaults: &HostBlock) -> SshProfile {
    let host = block.host_name.or_else(|| defaults.host_name.clone()).unwrap_or_else(|| name.clone());
    let port = block.port.or(defaults.port).unwrap_or(22);
    let user = block
        .user
        .or_else(|| defaults.user.clone())
        .unwrap_or_else(|| std::env::var("USERNAME").unwrap_or_else(|_| "root".into()));
    let key_path = block.identity_file.or_else(|| defaults.identity_file.clone());

    SshProfile {
        name,
        host,
        port,
        user,
        auth: "key".into(),
        password: None,
        key_path,
    }
}

#[derive(Default, Clone)]
struct HostBlock {
    host_name: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    identity_file: Option<String>,
}

fn strip_comment(line: &str) -> &str {
    if let Some(idx) = line.find('#') {
        &line[..idx]
    } else {
        line
    }
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    // Split on first whitespace or '='
    let bytes = line.as_bytes();
    let sep = bytes
        .iter()
        .position(|&b| b == b' ' || b == b'\t' || b == b'=')?;
    let key = &line[..sep];
    let rest = line[sep + 1..].trim_start_matches(['=', ' ', '\t']);
    Some((key, rest))
}
