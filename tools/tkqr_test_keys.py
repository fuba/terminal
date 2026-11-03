"""Generate disposable public test-fixture keys. NEVER use these for SSH access."""
from pathlib import Path
from cryptography.hazmat.primitives.asymmetric import ed25519, rsa
from cryptography.hazmat.primitives import serialization as s

root = Path(__file__).resolve().parents[1] / "docs" / "test-vectors" / "keys"
root.mkdir(parents=True, exist_ok=True)
for algorithm, key in [("ed25519", ed25519.Ed25519PrivateKey.generate()), ("rsa3072", rsa.generate_private_key(65537, 3072)), ("rsa4096", rsa.generate_private_key(65537, 4096))]:
    for format, kind in [("openssh", s.PrivateFormat.OpenSSH), ("pem", s.PrivateFormat.PKCS8)]:
        for encrypted in [False, True]:
            encryption = s.BestAvailableEncryption(b"test-only-do-not-use") if encrypted else s.NoEncryption()
            data = key.private_bytes(s.Encoding.PEM, kind, encryption)
            name = f"{algorithm}-{format}-{'encrypted' if encrypted else 'plain'}.txt"
            (root / name).write_bytes(data)
    if algorithm.startswith("rsa"):
        (root / f"{algorithm}-legacy-encrypted.txt").write_bytes(key.private_bytes(s.Encoding.PEM, s.PrivateFormat.TraditionalOpenSSL, s.BestAvailableEncryption(b"test-only-do-not-use")))
