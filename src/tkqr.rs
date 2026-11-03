//! TKQR1 revision 1 sender. No filesystem, UI, logging or clipboard access.
use aes_gcm::{
    aead::{AeadInPlace, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_KEY: usize = 16_384;
pub const MAX_JSON: usize = 24_576;

fn decode_armor(text: &str) -> Result<(&str, Zeroizing<Vec<u8>>), &'static str> {
    let mut lines = text.lines();
    let label = lines
        .next()
        .and_then(|s| s.strip_prefix("-----BEGIN "))
        .and_then(|s| s.strip_suffix("-----"))
        .ok_or("Select an OpenSSH or PEM private key file.")?;
    let end = format!("-----END {label}-----");
    let mut encoded = Zeroizing::new(String::with_capacity(text.len()));
    let mut ended = false;
    for line in lines {
        if ended {
            return Err("Select exactly one private key.");
        }
        if line == end {
            ended = true;
        } else {
            encoded.push_str(line);
        }
    }
    if !ended {
        return Err("Invalid private key footer.");
    }
    let der = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| "Invalid private key encoding.")?;
    Ok((label, Zeroizing::new(der)))
}

/// Recognize a single exportable key without decrypting or changing its bytes.
/// Encrypted PEM can only be fully validated by the receiver after decryption.
pub fn key_format(bytes: &[u8]) -> Result<&'static str, &'static str> {
    if bytes.is_empty() || bytes.len() > MAX_KEY {
        return Err("Select one private key file of 1–16384 bytes.");
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| "Select an OpenSSH or PEM private key file.")?;
    let text = text.trim();
    if text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----") {
        let (_, der) = decode_armor(text)?;
        let key =
            ssh_key::PrivateKey::from_bytes(&der).map_err(|_| "Invalid OpenSSH private key.")?;
        return match key.algorithm() {
            ssh_key::Algorithm::Ed25519 | ssh_key::Algorithm::Rsa { .. } => Ok("openssh"),
            _ => Err("Only file-backed Ed25519 and RSA keys are supported; FIDO/security keys cannot be exported."),
        };
    }
    if text.starts_with("-----BEGIN RSA PRIVATE KEY-----")
        && text.contains("Proc-Type: 4,ENCRYPTED")
    {
        let mut lines = text.lines();
        lines.next();
        if lines.next() != Some("Proc-Type: 4,ENCRYPTED") {
            return Err("Invalid encrypted PEM header.");
        }
        let dek = lines
            .next()
            .and_then(|s| s.strip_prefix("DEK-Info: "))
            .ok_or("Invalid PEM cipher header.")?;
        let (cipher, iv) = dek.split_once(',').ok_or("Invalid PEM cipher header.")?;
        let block = match cipher {
            "AES-128-CBC" | "AES-192-CBC" | "AES-256-CBC" => 16,
            "DES-EDE3-CBC" => 8,
            _ => return Err("Unsupported encrypted PEM cipher."),
        };
        if iv.len() != block * 2
            || !iv.bytes().all(|c| c.is_ascii_hexdigit())
            || lines.next() != Some("")
        {
            return Err("Invalid encrypted PEM header.");
        }
        let mut body = Zeroizing::new(String::with_capacity(text.len()));
        let mut ended = false;
        for line in lines {
            if line == "-----END RSA PRIVATE KEY-----" {
                ended = true;
                continue;
            }
            if ended {
                return Err("Select exactly one private key.");
            }
            body.push_str(line);
        }
        let decoded = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(body.as_bytes())
                .map_err(|_| "Invalid PEM data.")?,
        );
        if !ended || decoded.is_empty() || decoded.len() % block != 0 {
            return Err("Invalid encrypted PEM data.");
        }
        return Ok("pem");
    }
    let (label, der) = decode_armor(text)?;
    if label == "ENCRYPTED PRIVATE KEY" {
        pkcs8::EncryptedPrivateKeyInfo::try_from(der.as_slice())
            .map_err(|_| "Invalid encrypted PKCS#8 key.")?;
        return Ok("pem");
    }
    if !matches!(label, "RSA PRIVATE KEY" | "PRIVATE KEY") {
        return Err("Select an Ed25519 or RSA private key.");
    }
    russh_keys::decode_secret_key(text, None)
        .map_err(|_| "Invalid or unsupported PEM private key.")?;
    Ok("pem")
}

pub struct Transfer {
    pub passphrase: Zeroizing<String>,
    pub frames: Vec<String>,
}

impl Drop for Transfer {
    fn drop(&mut self) {
        self.frames.zeroize();
    }
}

pub fn payload(name: &str, format: &str, key: &[u8]) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err("Display name must be 1–128 UTF-8 bytes with no control characters.");
    }
    if !matches!(format, "openssh" | "pem") || key.is_empty() || key.len() > MAX_KEY {
        return Err("Private key must be 1–16384 bytes in OpenSSH or PEM format.");
    }
    let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(key));
    // Reserve before copying secrets: reallocating a growing String can leave
    // an unwiped former allocation behind even when the final buffer is wiped.
    let mut json = Zeroizing::new(Vec::with_capacity(MAX_JSON));
    json.extend_from_slice(b"{\"kind\":\"ssh-private-key\",\"name\":");
    json.extend_from_slice(
        serde_json::to_string(name)
            .map_err(|_| "Invalid display name")?
            .as_bytes(),
    );
    json.extend_from_slice(b",\"format\":\"");
    json.extend_from_slice(format.as_bytes());
    json.extend_from_slice(b"\",\"key\":\"");
    json.extend_from_slice(encoded.as_bytes());
    json.extend_from_slice(b"\"}");
    if json.len() > MAX_JSON {
        return Err("Payload is too large.");
    }
    Ok(json)
}

pub fn create(name: &str, format: &str, key: &[u8]) -> Result<Transfer, &'static str> {
    let json = payload(name, format, key)?;
    let mut random = Zeroizing::new([0u8; 62]);
    OsRng
        .try_fill_bytes(random.as_mut())
        .map_err(|_| "OS random generator failed.")?;
    let passphrase = Zeroizing::new(URL_SAFE_NO_PAD.encode(&random[..18]));
    let id: [u8; 16] = random[18..34].try_into().unwrap();
    let salt: [u8; 16] = random[34..50].try_into().unwrap();
    let nonce: [u8; 12] = random[50..62].try_into().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "System clock is before Unix epoch.")?
        .as_secs();
    let envelope = encrypt(&json, &passphrase, id, salt, nonce, now)?;
    Ok(Transfer {
        passphrase,
        frames: frames(&id, &envelope)?,
    })
}

fn encrypt(
    json: &[u8],
    passphrase: &str,
    id: [u8; 16],
    salt: [u8; 16],
    nonce: [u8; 12],
    now: u64,
) -> Result<Vec<u8>, &'static str> {
    if json.is_empty()
        || json.len() > MAX_JSON
        || passphrase.len() != 24
        || !passphrase
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("Invalid TKQR1 input.");
    }
    let expires = now.checked_add(300).ok_or("Invalid timestamp.")?;
    let mut header = Vec::with_capacity(69);
    header.extend_from_slice(b"TKQR\x01");
    header.extend_from_slice(&id);
    header.extend_from_slice(&now.to_be_bytes());
    header.extend_from_slice(&expires.to_be_bytes());
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    header.extend_from_slice(&(json.len() as u32).to_be_bytes());
    let mut derived = Zeroizing::new([0u8; 32]);
    pbkdf2::pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &salt, 600_000, derived.as_mut());
    let cipher = Aes256Gcm::new_from_slice(derived.as_ref()).map_err(|_| "Encryption failed.")?;
    let mut ciphertext = Zeroizing::new(json.to_vec());
    let tag = cipher
        .encrypt_in_place_detached(Nonce::from_slice(&nonce), &header, &mut ciphertext)
        .map_err(|_| "Encryption failed.")?;
    header.extend_from_slice(&ciphertext);
    header.extend_from_slice(&tag);
    Ok(header)
}

fn frames(id: &[u8; 16], envelope: &[u8]) -> Result<Vec<String>, &'static str> {
    if envelope.is_empty() || envelope.len() > 69 + MAX_JSON + 16 {
        return Err("Envelope too large.");
    }
    let encoded = URL_SAFE_NO_PAD.encode(envelope);
    let hash: String = Sha256::digest(envelope)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let id = URL_SAFE_NO_PAD.encode(id);
    let count = encoded.len().div_ceil(640);
    Ok(encoded
        .as_bytes()
        .chunks(640)
        .enumerate()
        .map(|(i, c)| {
            format!(
                "TKQR1/{id}/{}/{count}/{hash}/{}",
                i + 1,
                std::str::from_utf8(c).unwrap()
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_test_key_files_preserve_original_bytes() {
        for bytes in [
            include_bytes!("../docs/test-vectors/keys/ed25519-openssh-plain.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/ed25519-openssh-encrypted.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/ed25519-pem-plain.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/ed25519-pem-encrypted.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa3072-openssh-plain.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa3072-openssh-encrypted.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa3072-pem-plain.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa3072-pem-encrypted.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa4096-openssh-plain.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa4096-openssh-encrypted.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa4096-pem-plain.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa4096-pem-encrypted.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa3072-legacy-encrypted.txt").as_slice(),
            include_bytes!("../docs/test-vectors/keys/rsa4096-legacy-encrypted.txt").as_slice(),
        ] {
            let format = key_format(bytes).unwrap();
            let original = bytes.to_vec();
            let json = payload("テスト鍵", format, bytes).unwrap();
            let encrypted = encrypt(
                &json,
                "AAAAAAAAAAAAAAAAAAAAAAAA",
                [1; 16],
                [2; 16],
                [3; 12],
                0,
            )
            .unwrap();
            let plain = decrypt(&encrypted, "AAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
            let value: serde_json::Value = serde_json::from_slice(&plain).unwrap();
            assert_eq!(
                URL_SAFE_NO_PAD
                    .decode(value["key"].as_str().unwrap())
                    .unwrap(),
                original
            );
            assert_eq!(bytes, original);
        }
        assert!(key_format(b"ssh-ed25519 AAAA public-key-only").is_err());
        assert!(key_format(
            b"-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----"
        )
        .is_err());
    }

    fn unhex(s: &str) -> Vec<u8> {
        s.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
            .collect()
    }

    fn decrypt(envelope: &[u8], password: &str) -> Result<Vec<u8>, ()> {
        if envelope.len() < 86 || &envelope[..5] != b"TKQR\x01" {
            return Err(());
        }
        let n = u32::from_be_bytes(envelope[65..69].try_into().unwrap()) as usize;
        if !(1..=MAX_JSON).contains(&n) || envelope.len() != 85 + n {
            return Err(());
        }
        let mut key = Zeroizing::new([0u8; 32]);
        pbkdf2::pbkdf2_hmac::<Sha256>(
            password.as_bytes(),
            &envelope[37..53],
            600_000,
            key.as_mut(),
        );
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).unwrap();
        let mut body = envelope[69..69 + n].to_vec();
        cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&envelope[53..65]),
                &envelope[..69],
                &mut body,
                aes_gcm::Tag::from_slice(&envelope[69 + n..]),
            )
            .map_err(|_| ())?;
        Ok(body)
    }

    #[test]
    fn matches_independent_python_vectors_and_authenticates() {
        for fixture in [
            include_str!("../docs/test-vectors/tkqr1-single.json"),
            include_str!("../docs/test-vectors/tkqr1-multi.json"),
        ] {
            let v: serde_json::Value = serde_json::from_str(fixture).unwrap();
            let get = |k: &str| v[k].as_str().unwrap();
            let plain = unhex(get("plaintext_hex"));
            let aad = unhex(get("aad_hex"));
            let envelope = encrypt(
                &plain,
                get("passphrase"),
                aad[5..21].try_into().unwrap(),
                aad[37..53].try_into().unwrap(),
                aad[53..65].try_into().unwrap(),
                u64::from_be_bytes(aad[21..29].try_into().unwrap()),
            )
            .unwrap();
            assert_eq!(envelope, unhex(get("envelope_hex")));
            assert_eq!(&envelope[..69], aad);
            assert_eq!(
                &envelope[69..envelope.len() - 16],
                unhex(get("ciphertext_hex"))
            );
            assert_eq!(&envelope[envelope.len() - 16..], unhex(get("tag_hex")));
            let generated = frames(&aad[5..21].try_into().unwrap(), &envelope).unwrap();
            assert_eq!(
                generated,
                v["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|f| f.as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            );
            assert_eq!(decrypt(&envelope, get("passphrase")).unwrap(), plain);
            let decoded: serde_json::Value = serde_json::from_slice(&plain).unwrap();
            assert_eq!(decoded["name"], "テスト鍵");
            assert_eq!(
                URL_SAFE_NO_PAD
                    .decode(decoded["key"].as_str().unwrap())
                    .unwrap(),
                unhex(get("original_hex"))
            );
        }
    }

    #[test]
    fn rejects_wrong_password_tamper_version_and_lengths() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../docs/test-vectors/tkqr1-single.json")).unwrap();
        let envelope = unhex(v["envelope_hex"].as_str().unwrap());
        let password = v["passphrase"].as_str().unwrap();
        assert!(decrypt(&envelope, "AAAAAAAAAAAAAAAAAAAAAAAA").is_err());
        for index in [4, 5, 21, 29, 37, 53, 65, 68, 69, envelope.len() - 1] {
            let mut bad = envelope.clone();
            bad[index] ^= 1;
            assert!(decrypt(&bad, password).is_err(), "index {index}");
        }
        for len in [0, 4, 68, 69, envelope.len() - 1] {
            assert!(decrypt(&envelope[..len], password).is_err());
        }
        let mut extra = envelope;
        extra.push(0);
        assert!(decrypt(&extra, password).is_err());
    }

    #[test]
    fn regeneration_uses_fresh_randomness_and_preserves_bytes() {
        let bytes = b"test only\r\n";
        let a = create("鍵", "pem", bytes).unwrap();
        let b = create("鍵", "pem", bytes).unwrap();
        assert_eq!(a.passphrase.len(), 24);
        assert_ne!(a.passphrase, b.passphrase);
        assert_ne!(a.frames, b.frames);
        assert!(encrypt(b"x", "short", [0; 16], [0; 16], [0; 12], 0).is_err());
        assert!(encrypt(
            b"x",
            "AAAAAAAAAAAAAAAAAAAAAAAA",
            [0; 16],
            [0; 16],
            [0; 12],
            u64::MAX
        )
        .is_err());
    }

    #[test]
    fn payload_boundaries_and_names() {
        assert!(payload("鍵", "openssh", &[1]).is_ok());
        for name in ["", "bad\nname"] {
            assert!(payload(name, "openssh", &[1]).is_err());
        }
        assert!(payload(&"あ".repeat(43), "pem", &[1]).is_err());
        assert!(payload("key", "pem", &[]).is_err());
        assert!(payload("key", "pem", &vec![0; 16_384]).is_ok());
        assert!(payload("key", "pem", &vec![0; 16_385]).is_err());
    }

    #[test]
    fn frame_boundaries() {
        for len in [1, 479, 480, 481, 24_661] {
            let bytes = vec![42; len];
            let frames = frames(&[0; 16], &bytes).unwrap();
            let encoded = frames
                .iter()
                .map(|s| s.rsplit('/').next().unwrap())
                .collect::<String>();
            assert_eq!(URL_SAFE_NO_PAD.decode(encoded).unwrap(), bytes);
            assert!(frames.iter().all(|s| s.len() <= 768));
            assert!(frames[..frames.len() - 1]
                .iter()
                .all(|s| s.rsplit('/').next().unwrap().len() == 640));
        }
        assert!(frames(&[0; 16], &vec![0; 24_662]).is_err());
    }
}
