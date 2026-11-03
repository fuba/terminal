"""Generate public, synthetic TKQR1 fixtures with an independent implementation.

Requires Python cryptography. Never use real keys with this script.
"""
import base64
import hashlib
import json
from pathlib import Path
from cryptography.hazmat.primitives.ciphers.aead import AESGCM


def b64(value):
    return base64.urlsafe_b64encode(value).decode().rstrip("=")


def vector(size):
    original = (b"SYNTHETIC TEST DATA - NOT A PRIVATE KEY\n" * 500)[:size]
    plaintext = json.dumps({"kind": "ssh-private-key", "name": "テスト鍵", "format": "openssh", "key": b64(original)}, ensure_ascii=False, separators=(",", ":")).encode()
    transfer_id = bytes(range(16))
    salt = bytes(range(16, 32))
    nonce = bytes(range(32, 44))
    passphrase = b64(bytes(range(44, 62)))
    created = 1735689600
    aad = b"TKQR\x01" + transfer_id + created.to_bytes(8, "big") + (created + 300).to_bytes(8, "big") + salt + nonce + len(plaintext).to_bytes(4, "big")
    derived = hashlib.pbkdf2_hmac("sha256", passphrase.encode(), salt, 600000, 32)
    ciphertext_tag = AESGCM(derived).encrypt(nonce, plaintext, aad)
    envelope = aad + ciphertext_tag
    digest = hashlib.sha256(envelope).hexdigest()
    encoded = b64(envelope)
    chunks = [encoded[i:i+640] for i in range(0, len(encoded), 640)]
    frames = [f"TKQR1/{b64(transfer_id)}/{i+1}/{len(chunks)}/{digest}/{chunk}" for i, chunk in enumerate(chunks)]
    return dict(note="Public synthetic test data, never use these random values in production", original_hex=original.hex(), plaintext_hex=plaintext.hex(), passphrase=passphrase, aad_hex=aad.hex(), ciphertext_hex=ciphertext_tag[:-16].hex(), tag_hex=ciphertext_tag[-16:].hex(), envelope_hex=envelope.hex(), sha256=digest, frames=frames)


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[1] / "docs" / "test-vectors"
    root.mkdir(parents=True, exist_ok=True)
    for name, size in [("single", 1), ("multi", 2048)]:
        (root / f"tkqr1-{name}.json").write_text(json.dumps(vector(size), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
