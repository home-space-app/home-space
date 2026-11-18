# Protected folders: end-to-end encryption with a security key

## Problem

Everything a user stores is readable by whoever runs, backs up or breaks into the server: file
content on disk (or in object storage, see `object-storage.md`), names in `nodes`, and extracted
text in `search_entries`. Some folders (identity documents, tax papers, medical records, keys)
should be readable only on the user's own devices, and only while the user has their hardware key
with them.

The user picks a folder and clicks **Protect**. From then on:

- The server never sees the content or the names of anything inside it. Encryption and decryption
  happen in the client; the server stores ciphertext.
- The folder can be browsed, and its files opened or downloaded, only from a client with one of
  the user's security keys (a YubiKey) inserted and unlocked with its PIN.
- Losing a key does not mean losing the data: a **backup key is required**.
- Protected files are **not visible in the on-demand files** (Windows Cloud Files, macOS File
  Provider, Linux FUSE, the mobile file providers) nor in folder sync.
- Desktop first, mobile later. The user asked whether the **web client in Chrome** could work too
  (talk to the key, decrypt in memory, hand files out through the File API). It can, with a trust
  caveat; see [Web client](#web-client).

## Summary of the proposal

| Question | Proposal |
|---|---|
| What the hardware key does | Derive a secret with the **FIDO2 `hmac-secret` extension** (WebAuthn **PRF** in browsers). One mechanism for desktop, web, Android and iOS. |
| Why not RSA (PIV / OpenPGP) | Browsers cannot reach PIV or OpenPGP applets. PRF works in Chrome, Edge and Firefox, and through raw CTAP2 on every native platform. |
| Key hierarchy | Security key → **vault key** (one per user) → **folder key** (one per protected folder) → per-file keys and the name key. |
| Backup | At least **two security keys** are required before the first folder can be protected; a printed **recovery key** is offered too. Each one wraps the same vault key. |
| Content format | [age](https://age-encryption.org/v1) v1 (ChaCha20-Poly1305 STREAM, 64 KiB chunks, random file key per file and per version), via the `age` crate. |
| Names | Deterministic AES-SIV with the parent folder id as associated data, so the server can still enforce unique names per folder without reading them. |
| Where the server is involved | Stores ciphertext and wrapped keys, marks protected subtrees, refuses plaintext into them, and leaves them out of search, sharing, previews, on-demand files and sync. |
| Clients | Desktop (iced) first, then web (Leptos/WASM, opt-in), then Android and iOS. All share one Rust crate, `clients/vault-core`. |

## Threat model

Protects against:

- Someone who reads the server's disk, database, object storage bucket or backups.
- A server operator, or an attacker with the server, who reads data passively.
- Someone who steals a laptop with the desktop app signed in: without the security key and its
  PIN the protected folders cannot be opened.
- Someone who steals the security key: without its PIN the key does not answer (FIDO2 user
  verification is required, and the key locks itself after 8 wrong PINs).

Does **not** protect against:

- An attacker controlling a client while it is unlocked (malware on the desktop, a malicious
  browser extension). They can read what the user can read.
- A server that serves a **modified web client**. This only affects the web client; see
  [Web client](#web-client).
- Metadata. The server still sees: which folder is protected, how many files and folders it has
  and how they nest, file sizes (rounded only by the 64 KiB chunks plus a small header), when
  files are created and changed, which device changed them, and which files have equal names
  only in the narrow sense below. It does not see names, MIME types or content.
- Deletion. The server can still delete or roll back ciphertext. Tampering is detected (AEAD), but
  the server can withhold a file or serve an older version of it. Rollback detection is listed
  under [Later](#later).

## The hardware key: PRF instead of RSA

The original ask said "RSA". There are three ways a YubiKey can protect a key:

| | FIDO2 `hmac-secret` / WebAuthn PRF | PIV (RSA / ECC, slot 9d) | OpenPGP card |
|---|---|---|---|
| Browser (Chrome, Edge, Firefox) | **Yes**, WebAuthn PRF | No | No |
| Desktop (Linux, macOS, Windows) | Yes: CTAP2 over HID; on Windows through `webauthn.dll` | Yes, PC/SC | Yes, PC/SC or gpg-agent |
| Android, iOS | Yes, Yubico's yubikit SDKs (USB-C, NFC) | Yes, yubikit | Partly |
| YubiKey models | YubiKey 5, Security Key series (all FIDO2 keys from Yubico), and keys from other vendors | YubiKey 5 only | YubiKey 5 only |
| Operation | Symmetric: key returns HMAC(secret on key, salt). Needs the key to **read and to write**. | Asymmetric: anyone can encrypt to the public key; decrypting needs the key. | Asymmetric |
| Setup on the key | Nothing; a non-discoverable credential uses no slot | Generate a key in a slot, management key | Generate keys, admin PIN |

**Recommendation: PRF (`hmac-secret`) as the only mechanism.** It is the only one that works in a
browser, it needs no setup on the key, it works with any FIDO2 key (not only YubiKey 5), and the
same credential gives the same secret on every client. PIV's real advantage, writing without the
key present, is recovered by giving every folder its own X25519 key pair (see below), so new files
could later be encrypted to a folder from a client that is not unlocked.

How a PRF secret is produced:

1. On enrolment the client creates a WebAuthn credential (`residentKey: discouraged`, so the key
   stores nothing; `userVerification: required`). The server stores the credential id, the RP id
   and a random 32-byte salt.
2. To unlock, the client runs an assertion with that credential id and salt. The user enters the
   PIN and touches the key. The key returns a 32-byte secret.
3. The client derives a wrapping key from it: `kek = HKDF-SHA256(secret, info="home-space vault kek v1")`.

Rules that every client has to get exactly right, or one client cannot open what another wrote:

- **Same RP id everywhere.** A credential is bound to its RP id. The RP id is the host of the
  server's public URL (the `public_url` the server already has for OIDC), fixed when the vault is
  created and stored with each credential. Native clients pass it as a plain string over CTAP2;
  browsers can only use it when the web client is served from that host over HTTPS. Opening the
  web client by IP address or another host name cannot unlock (WebAuthn does not allow IP RP ids).
- **The PRF salt transform.** Browsers do not hand the PRF input to the key as is: they send
  `SHA-256("WebAuthn PRF" || 0x00 || input)`. Native clients that speak `hmac-secret` directly
  must apply the same transform, so the PRF value matches the browser's.
- **Always user verification.** `hmac-secret` returns a different secret with and without UV. Every
  client always requires UV (PIN, or fingerprint on YubiKey Bio).

## Key hierarchy

```
security key A ─┐
security key B ─┼─ each wraps ──▶ vault key (32 bytes, one per user, random)
recovery key  ─┘                         │
                                          ├─ wraps ──▶ folder secret (32 bytes, one per protected folder)
                                          │                 ├─ HKDF ─▶ name key (AES-SIV-256)
                                          │                 └─ HKDF ─▶ X25519 identity (age); public key stored on the server
                                          │                                  │
                                          │                                  └─ age recipient ─▶ file key per file version
```

- **Vault key**: random, generated on the client when the user sets up protection. It never leaves
  a client unwrapped. Each unlocker (security key or recovery key) stores its own copy of it,
  wrapped with XChaCha20-Poly1305 under the unlocker's `kek`. Adding a backup key while unlocked
  wraps the vault key once more; nothing else is re-encrypted.
- **Folder secret**: random per protected folder, wrapped by the vault key. A separate key per
  folder lets a folder be re-keyed or (later) shared with another user without touching the rest.
- **Name key** and **X25519 identity**: derived from the folder secret with HKDF and distinct
  `info` strings. The X25519 public key (the folder's age recipient) is stored in clear on the
  server.
- **File key**: age generates a random one for every file it encrypts, so every version of every
  file has its own. It is wrapped to the folder's recipient in the age header, inside the blob.

### Backup key and recovery

"Losing the key means losing the data" is true for a single key, so:

- Setting up protection asks for **two security keys** (the second one can be inserted right
  after the first). Protect stays disabled until two are enrolled. The UI says plainly that the
  backup key should be stored somewhere else.
- A **recovery key** is offered at setup and can be created later: 32 random bytes shown once as
  a grouped Base32 string (and a QR code) for printing. It is a third unlocker, with
  `kek = HKDF(recovery bytes)`; no PRF. It is optional: a user who wants hardware only can skip it.
  **Open question:** make it required instead of optional.
- **Key management** (desktop settings, "Security keys"): list unlockers with names and last use,
  add a key, remove a key (never the last two security keys; removing needs an unlock with
  another unlocker), replace the recovery key.
- Removing a lost key deletes its wrapped vault key on the server, so the lost key cannot unlock
  any more. Because the vault key itself does not change, someone who copied the wrapped vault key
  and has the lost key and its PIN could still unlock. **Rotate vault key** (re-wrap every folder
  secret with a new vault key, a few KB of work, not re-encrypting files) runs automatically when
  a key is removed.

## What is encrypted where

### Content: age

Each file is an age v1 file with one X25519 recipient, the folder's:

- Header: the file key wrapped to the folder recipient, plus a MAC. ~200 bytes.
- Payload: ChaCha20-Poly1305 in 64 KiB chunks with a counter nonce and a last-chunk flag, so
  truncation, reordering and chunk swapping are detected. A chunk can be decrypted on its own,
  which allows range reads (video seeking in the viewer) without downloading the whole file.

Why age rather than a home-grown AES-GCM format: the format is specified, has two independent
implementations (`age`/`rage` in Rust, `age` in Go), and is already chunked and seekable. The
`age` crate builds for WASM. ChaCha20-Poly1305 is constant-time in software, which matters in
WASM and on phones without AES instructions; AES-256-GCM brings nothing here. It also means a
user can decrypt an exported file with the standard `age` tool once they export the folder's
identity (an emergency "export keys" action, behind an unlock).

The server sees the blob's ciphertext size and its SHA-256 (it still hashes uploads to verify
them). Because file keys are random, equal files have different ciphertexts, so the server learns
nothing from equal hashes.

### Names: deterministic AES-SIV

The server enforces `UNIQUE (owner_id, parent_id, kind, name)` and the clients rely on 409
conflicts for "keep both / override / skip". Random-nonce encryption would hide duplicates from
the server. AES-SIV (RFC 5297, `aes-siv` crate) is deterministic: the same name in the same folder
gives the same ciphertext, so the existing uniqueness check keeps working.

- `sealed_name = AES-SIV(name key, ad = [parent_id, kind], plaintext = name as UTF-8)`.
  With the parent id as associated data, the same name in two folders gives two unrelated
  ciphertexts; the server learns only what it already enforces: that names within one folder
  differ.
- The plaintext is padded to a multiple of 32 bytes before sealing, so the length of a name is
  hidden to that granularity.
- Stored in a new `sealed_name BYTEA` column (up to 1 KiB). The existing `name` column holds the
  node id as text for sealed nodes, which keeps all its checks and the constraint satisfied, and is
  what an old client would show. A unique index on `(owner_id, parent_id, kind, sealed_name)` where
  `sealed_name IS NOT NULL` replaces the name constraint for sealed nodes.
- **Case:** names compare by bytes, as today. Unicode is normalized to NFC before sealing, so
  macOS (NFD) and the others agree.
- A file's MIME type is not sent; the server stores `application/octet-stream`. The client derives
  it from the decrypted name.

**The protected folder's own name** stays in clear, so the user sees "Taxes 🔒" in the listing
without the key. Everything inside it is sealed. **Open question:** offer to seal the root name
too (it would show as "Protected folder" until unlocked).

## Server changes

The server does no cryptography. It stores what clients give it and keeps protected data away
from features that would need plaintext.

### Schema

```sql
-- The user's vault: one per user, created by the first "Set up protection".
CREATE TABLE vaults (
    user_id    UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- Bumped by "rotate vault key"; folder secrets carry the epoch they are wrapped under.
    epoch      INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- What can unlock the vault: security keys and the recovery key.
CREATE TABLE vault_unlockers (
    id            UUID PRIMARY KEY,
    user_id       UUID NOT NULL REFERENCES vaults (user_id) ON DELETE CASCADE,
    kind          TEXT NOT NULL CHECK (kind IN ('fido2', 'recovery')),
    label         TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 64),
    credential_id BYTEA,         -- fido2 only
    rp_id         TEXT,          -- fido2 only
    prf_salt      BYTEA,         -- fido2 only, 32 bytes
    wrapped_vault_key BYTEA NOT NULL,
    epoch         INT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ,
    CHECK ((kind = 'fido2') = (credential_id IS NOT NULL AND rp_id IS NOT NULL AND prf_salt IS NOT NULL))
);

-- One row per protected folder (the folder the user clicked Protect on).
CREATE TABLE protected_folders (
    node_id        UUID PRIMARY KEY REFERENCES nodes (id) ON DELETE CASCADE,
    owner_id       UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    wrapped_secret BYTEA NOT NULL,
    epoch          INT NOT NULL,
    recipient      TEXT NOT NULL,      -- age X25519 public key, "age1..."
    format         INT NOT NULL DEFAULT 1,
    -- 'protecting' while an existing folder is being converted, see below.
    state          TEXT NOT NULL CHECK (state IN ('protecting', 'protected')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE nodes ADD COLUMN sealed BOOLEAN NOT NULL DEFAULT false;  -- inside a protected folder
ALTER TABLE nodes ADD COLUMN sealed_name BYTEA CHECK (length(sealed_name) <= 1024);
```

`sealed` is set on every node below a protected folder (not on the folder itself). It is
denormalized so that every query that has to skip protected data (search, shares, listings for
VFS, changes) can filter on one column instead of walking ancestors.

### API

- `GET/POST/DELETE /vault`, `/vault/unlockers`, `/vault/unlockers/{id}`: read the vault (credential
  ids, salts, wrapped keys; all useless without a security key), add and remove unlockers, rotate.
  Removing refuses to go below two `fido2` unlockers. Changing unlockers requires a fresh sign-in
  (like `/me/password`) so a stolen access token cannot add an attacker's unlocker.
- `POST /protected` creates a protected folder (new, or for an existing folder; see below).
  `GET /protected` lists them with their wrapped secrets.
- `POST /folders`, `POST /files`, `POST /uploads`, `PATCH /nodes/{id}` and `POST /nodes/move` take
  `sealed_name` instead of `name` for nodes inside a protected folder. The server **rejects plaintext
  names and plaintext uploads** into a protected folder (`422 sealed_required`), so a client bug or
  an old client cannot leak data into it. It cannot check that content is encrypted, but it checks
  that it starts with the age header line.
- `capabilities` gains `protected_folders: true`.

### Moves

A sealed name is bound to its parent, so moving a node within a protected folder means the client
sends the newly sealed name with the move (`MoveRequest` gets an optional `sealed_names` map by
node id). Only the moved nodes change; their children keep theirs, since their parent did not
change. Merging on conflict works as today, on sealed names.

Moving **into or out of** a protected folder, or between two protected folders, is refused by the
server (`409 protection_boundary`). The client offers it as a copy that encrypts (or decrypts),
followed by a purge of the source, the same steps as Protect below.

### Features that need plaintext

| Feature | For sealed nodes |
|---|---|
| Search (`search_entries`, content providers) | Not indexed; existing entries deleted on protect. Client-side search inside an unlocked folder, over the decrypted names in memory, comes in phase 2. |
| Previews, gallery, viewers | Only in the client, after decryption. |
| Share links, shared folders, `archive` downloads | Refused (`409 protected`) for a protected folder or anything inside it. Sharing encrypted folders with other users is under [Later](#later). |
| Send to device (transfers) | Refused in phase 1. A sealed file can later be sent as ciphertext to the user's own devices. |
| Copy (`/shared/{id}/copy`, server-side copy) | Within one protected folder: allowed (copies ciphertext; the client sends the new sealed name). Elsewhere: refused. |
| Dedup by hash | Not possible (random file keys); there is none today anyway. |
| Versions | Kept. Each version is its own age file with its own file key. |
| Recycle bin | Trashed sealed nodes keep `sealed` and their sealed name. The bin shows them as "Protected item" until the user unlocks; restore puts them back where they were (restoring to Lost & Found is refused, since that would move them out of protection). |
| Storage quota | Counts ciphertext size, which is slightly larger. |
| Changes feed, websocket events | Sent with `sealed: true` and the sealed name, so unlocked clients can update live. Filtered out for clients that do not send `?sealed=include` (see below). |

### Hiding from on-demand files, sync and old clients

All listing endpoints (`/nodes`, `/changes`, `/ws` events, `/trash`) leave out sealed nodes unless
the request asks for them with `sealed=include`. Only the vault-aware views of the desktop,
web and mobile apps ask for them.

- The VFS adapters (`clients/vfs-core`, the macOS and mobile file providers) never pass
  `sealed=include`. The protected folder itself is not sealed, so it would still show as an empty
  folder there; vfs-core also hides any node that is a protected folder (the listing includes a
  `protected: true` flag on it). Result: **protected folders are not visible in the on-demand
  files at all**, as required.
- `sync-core` likewise skips protected folders. A folder sync mapping cannot point at or contain
  one; if a mapped folder is protected later, sync stops for that subtree and the app says why.
- Mobile auto-upload cannot target a protected folder in phase 1.
- Old clients see neither the content of protected folders nor garbage names.

## Protecting an existing folder

The server never sees plaintext after protection, but before it, it did. So protecting an existing
folder is a conversion done by the client:

1. The user clicks **Protect** on a folder and unlocks with a security key. If the user has no
   vault yet, the setup (two keys, optional recovery key) runs first.
2. The client generates a folder secret and calls `POST /protected` with `source = folder id`. The
   server creates a **new hidden folder** next to it (`.protecting-{id}`, not shown in listings) in
   state `protecting`, and blocks changes to the source folder (uploads, renames, moves, deletes
   return `423 locked`) so it cannot change during the copy. It also checks the quota: the copy
   temporarily needs the folder's size again.
3. The client walks the source tree. For every folder it creates the sealed folder; for every file
   it downloads the **current version**, encrypts it with age while streaming, and uploads it with
   its sealed name (chunked uploads for large files, as today). Progress shows in the transfers
   panel. A local journal (in the app's data dir) records what is done, so a crash or a closed
   laptop resumes where it stopped.
4. Old versions: the user chooses **keep versions** (each version is downloaded, encrypted and
   uploaded as a version of the new file, in order) or **current version only** (default, faster).
5. When every file is uploaded, the client calls `POST /protected/{id}/finish`. In one transaction
   the server:
   - moves the new folder into the source's place and name and marks it `protected`;
   - **hard-deletes** the source tree: nodes, `file_versions`, `search_entries`, share items,
     transfers that reference it, and any of its files that were already in the recycle bin
     (found through the hash of each node the conversion copied; the client lists them for the
     user to include or not);
   - queues the blobs for deletion and deletes them right after commit (with object storage,
     through `blob_journal`).
6. The client tells the user what the server cannot erase: **server backups and snapshots, and
   object storage versioning**, may still hold the old plaintext until they expire.

Cancelling deletes the hidden folder and unlocks the source. Unprotecting is the same in reverse
(decrypt and upload into a plain folder, then purge the ciphertext), behind an unlock.

**Open question:** step 2 locks the source folder for the whole conversion, which can take hours
for a large folder. The alternative (allow changes, then a second pass over what changed) is more
work and can wait.

## Desktop client (phase 1)

New crate `clients/vault-core` (no I/O to the server, no UI) used by desktop, web and mobile-core:

- types for the vault, unlockers and protected folders; wrapping and unwrapping (`chacha20poly1305`,
  `hkdf`, `sha2`), recovery key encoding;
- `seal_name` / `open_name` (`aes-siv`, `unicode-normalization`);
- streaming encrypt and decrypt, and range decrypt, on top of `age`;
- an `Authenticator` trait (`create_credential`, `prf(credential, salt)`), implemented by each
  platform; a software fake for tests;
- all secrets in `zeroize`/`secrecy` types.

Desktop `Authenticator`:

- **Linux and macOS:** CTAP2 over USB HID with `hmac-secret`. Candidates: `ctap-hid-fido2` and
  `ctap-fido2` (both MIT; the latter documents `hmac-secret` and was tested on a YubiKey 5C), or
  `libfido2` through FFI. **Spike:** pick one, verify the PRF value matches Chrome's for the same
  credential and salt. Linux needs the usual udev rule for FIDO devices (shipped by `libfido2` and
  most distributions). The Flatpak build needs `--device=all` for hidraw access.
- **Windows:** apps without admin rights cannot open FIDO devices directly, so the client must go
  through the Windows WebAuthn API (`webauthn.dll`, `windows` crate, `Win32::Networking::WindowsWebServices`),
  which shows the system PIN and touch prompt and supports `hmac-secret` for security keys. **Spike:**
  confirm the minimum Windows version and that the output matches the browser's PRF.

UI:

- "Protect…" in a folder's context menu; a setup wizard on first use (insert key A, PIN, touch;
  insert key B, PIN, touch; recovery key shown with "I have printed or saved it").
- A protected folder shows a lock icon. Opening it while locked shows "Insert your security key"
  with an Unlock button. PIN entry is the app's own dialog on Linux and macOS, the system's on
  Windows.
- **Unlock scope:** one unlock opens the vault (all protected folders) for this app session.
  Auto-lock after 15 minutes without activity in a protected folder (setting), on sleep and screen
  lock, and on sign-out. Locking drops all keys and decrypted names from memory.
- **Open:** viewers (images, text, PDF, video) decrypt into memory. "Open with…" an external app
  needs a file on disk: it is decrypted into a private temporary directory (`$XDG_RUNTIME_DIR` on
  Linux, which is RAM; the app's temp dir on macOS and Windows), deleted on lock and on app exit,
  with a warning the first time. Saving back from an external app comes in phase 2 (watch the
  file, re-encrypt, upload as a new version).
- **Download** is an explicit export to a place the user picks; the app warns that the copy is
  not protected.
- **Upload** into an unlocked protected folder encrypts on the fly (drag and drop, file picker).

## Web client

The user asked whether Chrome could reach the key, decrypt in memory and hand the files out. It
can:

- **Key access:** `navigator.credentials.get` with the `prf` extension and the stored credential
  id. Works with security keys in Chrome and Edge (since 116), and in recent Firefox. Safari
  supports PRF for its own passkeys; for USB and NFC security keys it had a bug returning
  unusable output as of early 2026, so Safari is treated as unsupported until verified.
- **Decrypt:** `vault-core` compiles to WASM with the Leptos app; nothing needs WebCrypto.
- **Hand out:** viewers get `Blob` URLs (revoked on lock). Downloads stream through the File
  System Access API (`showSaveFilePicker`, Chrome and Edge) so a large file is never fully in
  memory; elsewhere a `Blob` download, limited to files that fit in memory (the web client shows
  the limit).

**The caveat:** the web client's code comes from the same server that the encryption is meant to
protect against. A server operator, or an attacker who controls the server, can serve a modified
client that sends the keys or the plaintext back after the user unlocks. Desktop and mobile apps
do not have this problem: they are signed releases that do not change when the server changes.
This is the same limit every web-based end-to-end encrypted service has.

Proposal: support the web client, but as an **opt-in per vault** ("Allow unlocking in the
browser", off by default, explained in one sentence). The web client checks the vault setting and
offers unlocking only when it is on. It does not stop a malicious server (which could ignore the
flag), but it makes the trade-off a decision the user takes, and an honest server respects it.
Phase 3, after desktop.

## Mobile (phase 4)

- **Android:** `yubikit-android` (FIDO2 over USB-C and NFC, with `hmac-secret`) through mobile-core.
  Using the raw CTAP2 session instead of Android Credential Manager avoids needing a Digital
  Asset Links file on every self-hosted server. Implemented in Kotlin, behind the same
  `Authenticator` trait exposed through UniFFI.
- **iOS:** Yubico's `yubikit-ios` (NFC, Lightning, USB-C). `ASAuthorization` security key APIs need
  an associated domain for the RP id, which cannot be listed at build time for self-hosted
  servers, so the raw CTAP2 path is the only one that works. **Spike** needed.
- Files open in the in-app viewers only; sharing a decrypted file to another app writes a
  temporary copy that is deleted when the share sheet closes.
- The mobile file providers never show protected folders (same `sealed=include` rule).

## Phases

1. **Foundation** (server + `vault-core`): schema, vault and protected folder API, sealed names,
   refusing plaintext, hiding from search, shares, transfers, VFS and sync; `vault-core` with tests
   against fixed vectors; Authenticator spikes for Linux and Windows.
2. **Desktop**: setup wizard, key management, Protect (new and existing folders), browse, open,
   upload, download, move and rename inside, auto-lock, recycle bin.
3. **Desktop polish**: external-app save-back, client-side search in an unlocked folder, keep
   versions on protect, unprotect.
4. **Web** (opt-in), with Chrome, Edge and Firefox in the e2e tests (a virtual authenticator with
   PRF in Chrome DevTools Protocol / Playwright).
5. **Android**, then **iOS**.

## Testing

- `vault-core`: fixed test vectors for the KEK derivation, PRF salt transform, name sealing and age
  files; property tests that seal/open and encrypt/decrypt round-trip, including range reads.
- A software authenticator (`soft-fido2` or our own fake) implements `hmac-secret` for tests and
  CI, so the server e2e tests (`e2e/`) can drive the whole flow without hardware.
- Cross-client check: a file protected by the desktop opens in Chrome with the same key, and the
  other way round (manual, with a real YubiKey, before each phase ships).
- Server tests: plaintext rejected inside protected folders, sealed nodes absent from search,
  shares, VFS listings and changes without `sealed=include`, purge removes every trace of the
  source tree (nodes, versions, search entries, bin, blobs).

## Later

- **Write without unlocking:** the folder's X25519 recipient is public, so a client could encrypt
  new files into a protected folder without the key (for example mobile auto-upload into a
  protected "Scans" folder). Names would be sealed to the recipient instead of with AES-SIV and
  re-sealed on the next unlock.
- **Sharing a protected folder with another user:** wrap the folder secret to that user's vault
  public key.
- **Rollback detection:** a signed, versioned manifest per folder so a client notices when the
  server withholds or rolls back files.
- **Seal the root name** (see open questions).
- **Synced passkeys** (Google Password Manager, iCloud Keychain, Windows Hello) as unlockers: PRF
  works with them too, but they are not hardware keys, and losing the account loses the
  passkey. Not offered in phase 1.

## Open questions

1. Recovery key: optional (proposed) or required?
2. Root folder name: stays in clear (proposed), or sealed too?
3. Unlock scope: whole vault for the session (proposed), or per folder?
4. Auto-lock timeout default: 15 minutes (proposed)?
5. Web client: opt-in per vault (proposed), or not at all?
6. Protecting an existing folder: lock the source during conversion (proposed), or allow changes
   and do a second pass?
7. Keep old versions when protecting: off by default (proposed)?
8. Minimum hardware: any FIDO2 key with `hmac-secret` (proposed), or YubiKey only?
