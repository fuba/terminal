# TKQR1 revision 1: Windows sender

Implements the sender described in [terminal #3](https://github.com/fuba/terminal/issues/3).
This is an application-specific transfer protocol, not an established standard or
an independently audited complete system. iPhone interoperability is **not yet
verified**. Do not close the issue on the basis of these sender tests alone.

## Operation

1. Open **+ → 秘密鍵をiPhoneへ転送…**. The transfer opens in a separate,
   resizable Win32 window on its own UI thread. Existing terminal sessions remain
   usable. No connection or authentication settings are changed.
2. Choose exactly one private key from the explicit app-profile `key_path` /
   SSH-config `IdentityFile` candidates, or use **ファイルを選択**. Multiple
   candidates require an explicit selection. This entry also works with no
   profiles. No key-directory scan is performed. The current SSH config importer
   does not implement `Include`, `Match`, or OpenSSH percent-token expansion;
   use the file picker for such paths.
3. Enter a display name (1–128 UTF-8 bytes, no control characters), select
   **生成 / 再生成**, and confirm the file and warning. Only the selected file is
   read, with a hard 16,384-byte limit. The original bytes, including line endings
   and any existing key encryption, are preserved.
4. Scan the QR(s) on the receiver. **パスフレーズ / QR** switches to the separate
   24-character transfer passphrase. Enter those exact case-sensitive ASCII
   characters on the iPhone. This is separate from the original key's passphrase.
   Nothing is automatically copied to the clipboard.
5. Multiple frames cycle every 350 ms. Pause/resume and previous/next are
   available; manually advancing pauses playback. Enlarge/maximize the window
   for a larger QR. M error correction, a four-module quiet zone and integer
   module sizing are used without image interpolation.
6. After five minutes, or on any session-change notification (including lock /
   disconnect), the transfer is cleared. Generate again to create a new
   passphrase, ID, salt and nonce. Closing clears the transfer as well.

The controls support normal Tab/Shift+Tab and the displayed Alt access keys;
Escape closes the window. Status and the currently revealed passphrase are native
accessible text. There is no image export/share action or passphrase QR.

## Implementation and security self-review

- `src/tkqr.rs` owns validation, the bounded JSON payload, PBKDF2-HMAC-SHA256
  (600,000), AES-256-GCM, the exact 69-byte AAD header and TKQR1 frame encoding.
  `src/app/key_transfer.rs` owns the native UI and monotonic lifetime.
- OS randomness (`rand::rngs::OsRng`) supplies 18 passphrase bytes plus separate
  16-byte ID, 16-byte salt and 12-byte nonce for every transfer. Fixed inputs are
  available only through private codec functions used by tests.
- RustCrypto AES-GCM performs encryption and authentication; no cryptographic
  primitive is implemented here. AES key-schedule zeroization is enabled.
- Original file buffers, decoded armor, base64 key material, JSON, the derived
  encryption key and passphrase use `Zeroizing`. Buffers reserve their bounds
  before copying secrets to avoid leaving old allocations after growth. Encrypted
  frame strings are wiped on drop; QR matrices are dropped. Library internals,
  Win32 static-control storage, register/stack copies, OS paging, screenshots and
  process dumps cannot be guaranteed to be erased by this application.
- No keys, QR frames or passphrases are passed to logging, config persistence,
  clipboard APIs, subprocesses, a WebView, or any network service. Errors shown
  by this feature are fixed descriptions, not parser debug dumps of input data.
- The passphrase text control is hidden and cleared during QR view. On switching
  to the passphrase, the prior QR is synchronously erased first. The receiver
  still needs to authenticate, validate the actual key and ask before saving.
- OpenSSH Ed25519/RSA keys are structurally parsed, including encrypted keys.
  FIDO/security-key algorithms, public-key files and unsupported formats are
  rejected. PEM supports unencrypted RSA/Ed25519 PKCS#8, RSA PKCS#1, encrypted
  PKCS#8, and legacy encrypted RSA PEM (AES-CBC / 3DES-CBC). For encrypted PEM,
  the sender can validate only the envelope, not the algorithm or private-key
  contents without the original passphrase; the receiver must validate after
  decrypting. The original passphrase is never requested or put in the payload.
- A photograph plus the transfer passphrase can still decrypt a transfer after
  the UI timeout. Expiry is not cryptographic revocation. The OS lock policy is
  not changed. Key reading and bounded crypto run synchronously in the dedicated
  transfer thread; notifications queued during that brief work are processed
  before normal subsequent interaction. The main terminal UI remains responsive.
- Native callbacks use `RefCell` to guard synchronous Win32 reentrancy. Session
  invalidation is recorded separately so a nested modal loop cannot lose it.
  No borrowed application-state pointer crosses the UI-thread boundary.

## Public cross-platform vectors

[`test-vectors/tkqr1-single.json`](test-vectors/tkqr1-single.json) and
[`test-vectors/tkqr1-multi.json`](test-vectors/tkqr1-multi.json) contain exact
plaintext bytes, passphrase, AAD, ciphertext, tag, full envelope, SHA-256 and every
frame. They were generated independently with Python `cryptography` AESGCM and
`hashlib.pbkdf2_hmac`, using `tools/tkqr_vector.py`. Rust compares every byte and
authenticates/decrypts both vectors. These are the revision-1 fixtures to share
with [terminal2 #6](https://github.com/fuba/terminal2/issues/6) and terminalkun2;
Swift implementation agreement / execution has not been verified here.

Files under `test-vectors/keys/` were generated solely for tests by
`tools/tkqr_test_keys.py`. **Never use these publicly committed private keys for
authentication.** Their original encryption password is `test-only-do-not-use`.
They cover Ed25519, RSA 3072 and RSA 4096; plain and encrypted OpenSSH / PEM,
plus legacy encrypted RSA PEM. The generators need Python `cryptography` and
`bcrypt`; the Rust application and normal Rust tests do not need Python.

## Validation

- `cargo test`: sender codec boundaries, exact independent vectors, Japanese
  display names, fresh transfer randomness, wrong passphrase, header/body/tag
  tampering, unknown version, truncation and extra data, plus byte-preserving
  round trips for all 14 generated private-key files.
- Native-window test: QR/passphrase visibility exclusion, pause and frame
  navigation, resize, injected WTS lock notification, monotonic expiry and close
  destruction. The test uses public fixtures only. It does not lock the user's
  workstation or touch real keys/config/clipboard.
  Run explicitly with `cargo test native_dialog_hides_secrets -- --ignored` on
  an interactive Windows desktop; it is excluded from headless CI by default.
- Visual inspection: native GDI client-area capture on Windows at 125% DPI,
  resized to the minimum supported layout. The rendered QR was independently
  decoded using ZXing-C++ and matched the complete first fixture frame exactly.
  [Public-fixture UI capture](test-vectors/tkqr1-native-ui.png).
  `TKQR_TEST_SCREENSHOT` is a **test-build-only** opt-in capture of that fixed
  public fixture. Production builds contain no capture/export implementation.
- Formatting, Clippy with warnings denied, unit tests and MSVC release build are
  required before installing. No hosted CI jobs were added or enabled; the
  repository's pre-existing workflow files are unchanged.

Remaining acceptance work: real OS lock/disconnect and five-minute wall-time UI
exercise; manual keyboard and screen-reader checks; additional physical DPI /
multi-monitor scenarios; Swift vector parity; receiver frame-collection negative
tests (out of order, missing/repeated/conflicting frames, mixed transfers,
canonical encodings, limits and clock-skew rules); iPhone camera collection of
single/multiple QR, Keychain save, fingerprint comparison and owned test-server
SSH authentication. No iPhone transfer completion claim is made.

## Dependency review (2026-11-04 JST)

Versions are pinned and `Cargo.lock` is committed. The newly downloaded packages
are qrcode 0.14.1 (2024-07-05), serde_json 1.0.145 (2025-09-14), itoa 1.0.18
(2026-03-20) and ryu 1.0.23 (2026-02-08). All were older than 24 hours when
selected, confirmed against crates.io version metadata. qrcode's optional image /
SVG / picture features are disabled, adding no QR runtime dependencies.

The additional direct dependencies already present transitively in the lockfile
are aes-gcm 0.10.3, aes 0.8.4, pbkdf2 0.12.2, sha2 0.10.9, rand 0.8.6,
zeroize 1.8.2, ssh-key 0.6.7 and pkcs8 0.10.2; their publication dates were also
checked and exceed 24 hours. Sources are crates.io checksum-verified packages.
Review covered manifests/features, QR encoding paths, OS randomness, crypto
in-place API / zeroization paths, key parsing APIs, bounded serialization and new
dependency I/O/build behavior. No networking or external commands are introduced
by the added encoder/serialization code. This was a focused source/supply review,
not a full audit of the repository's pre-existing dependency tree.
