# Protected folders: end-to-end encryption with a security key or a password

## Problem

Everything a user stores is readable by whoever runs, backs up or breaks into the server: file
content on disk (or in object storage, see `object-storage.md`), names in `nodes`, and extracted
text in `search_entries`. Some folders (identity documents, tax papers, medical records, keys)
should be readable only on the user's own devices.

The user picks a folder and clicks **Protect**, choosing one of two locks:

- **Security key** (recommended): one of the security keys (a YubiKey or any FIDO2 key) the user
  already enrolled for signing in (`local-accounts.md`). Someone with the server and all its data
  still cannot decrypt the folder: the secret is on the key, behind its PIN.
- **Password** (less secure): a password chosen for the folder. It works without a key, but
  someone with the server's data can try to guess it offline.

From then on:

- The server never sees the content or the names of anything inside the folder, current or past.
  Encryption and decryption happen in the client; the server stores ciphertext. Protecting an
  existing folder encrypts its files **and all their old versions** in place.
- The folder has two new operations, **Unlock** and **Lock**. Unlock shows the names. For a
  security-key folder **no decryption key stays in the client's memory**: opening, downloading or
  uploading a file needs another touch of the key. Folders lock themselves after inactivity and
  when the app closes.
- Nothing is forced: one security key is enough, with a plain warning that losing it loses the
  data.
- Protected files are **not visible in the on-demand files** (Windows Cloud Files, macOS File
  Provider, Linux FUSE, the mobile file providers) nor in folder sync.
- Desktop first, mobile later. The web client in Chrome can work too, opt-in and with a trust
  caveat; see [Web client](#web-client).

## Summary of the proposal

| Question | Proposal |
|---|---|
| Locks | Per folder: **security key** or **password**. The security-key choice is disabled until the account has an eligible key. |
| Which security keys | The account's existing WebAuthn credentials (`user_webauthn`) that live on a hardware key and support **PRF / `hmac-secret`**. Synced passkeys (iCloud Keychain, Google Password Manager) are not offered: native apps cannot reach them. |
| What the key does | Derives a secret with the **FIDO2 `hmac-secret` extension** (WebAuthn **PRF** in browsers). One mechanism for desktop, web, Android and iOS. |
| Key hierarchy | Security key → **vault key** (one per user) → **folder secret** (one per folder) → name key and age identity → file keys. Password → Argon2id → folder secret, with no vault in between. |
| Backup | Not required. One key gets a warning; a second key and a printed **recovery key** are offered. |
| What stays in memory | Security-key folders: the decrypted names and the key's PIN token, never a decryption key. Password folders: the derived key, until Lock. |
| Content format | [age](https://age-encryption.org/v1) v1 (ChaCha20-Poly1305 STREAM, 64 KiB chunks, random file key per file and per version), via the `age` crate. |
| Names | Deterministic AES-SIV with the parent folder id as associated data, so the server can still enforce unique names per folder without reading them. |
| Protecting an existing folder | In place: every version of every file is downloaded, encrypted and swapped on the server; node ids, version numbers and dates stay. |
| Where the server is involved | Stores ciphertext and wrapped keys, marks protected subtrees, refuses plaintext into them, and leaves them out of search, sharing, previews, on-demand files and sync. |
| Clients | Desktop (iced) first, then web (Leptos/WASM, opt-in), then Android and iOS. All share one Rust crate, `clients/vault-core`. |

## Threat model

Protects against:

- Someone who reads the server's disk, database, object storage bucket or backups, and a server
  operator or attacker with the server who reads data passively:
  - **Security-key folders:** fully. What the server holds (credential ids, salts, wrapped keys,
    ciphertext) is useless without the physical key and its PIN.
  - **Password folders:** only as far as the password resists guessing. The wrapped folder secret
    is on the server, so an attacker can try passwords offline as fast as their hardware runs
    Argon2id. A minimum length and a strength meter help; a security key is still the real
    protection, and the UI says so.
- Someone who steals a laptop with the desktop app signed in: a security-key folder cannot be
  opened without the key and its PIN; a password folder not without its password.
- Someone who steals the security key: without its PIN the key does not answer (FIDO2 user
  verification is required, and the key locks itself after 8 wrong PINs).
- Malware that reads the app's memory **while a security-key folder is unlocked but no file is
  being opened**: it finds names, not keys (see [Unlock and lock](#unlock-and-lock)).

Does **not** protect against:

- An attacker controlling a client while the user works in it (malware on the desktop, a
  malicious browser extension). They can see what the user opens, can wait for the next touch,
  and can read a password folder's key while it is unlocked.
- A server that serves a **modified web client**. This only affects the web client; see
  [Web client](#web-client).
- Metadata. The server still sees: which folder is protected and with which lock, how many files
  and folders it has and how they nest, file sizes (rounded only by the 64 KiB chunks plus a
  small header), how many versions each file has, when files are created and changed, which
  device changed them, and which files have equal names only in the narrow sense below. It does
  not see names, MIME types or content.
- Deletion. The server can still delete or roll back ciphertext. Tampering is detected (AEAD), but
  the server can withhold a file or serve an older version of it. Rollback detection is listed
  under [Later](#later).
- Plaintext from before protection that the server cannot erase: backups, snapshots, object
  storage versioning and freed disk blocks (see
  [Protecting an existing folder](#protecting-an-existing-folder)).

## Security keys: reusing the sign-in credentials

Passkeys and security keys for signing in already exist (`local-accounts.md`,
`server/migrations/0031_webauthn.sql`): one `user_webauthn` table, registered in the web
client, used by the native apps through browser sign-in. Protected folders reuse those
credentials instead of enrolling separate ones (decided 2026-11-26).

### Why PRF and not RSA

There are three ways a YubiKey can protect a key:

| | FIDO2 `hmac-secret` / WebAuthn PRF | PIV (RSA / ECC, slot 9d) | OpenPGP card |
|---|---|---|---|
| Browser (Chrome, Edge, Firefox) | **Yes**, WebAuthn PRF | No | No |
| Desktop (Linux, macOS, Windows) | Yes: CTAP2 over HID; on Windows through `webauthn.dll` | Yes, PC/SC | Yes, PC/SC or gpg-agent |
| Android, iOS | Yes, Yubico's yubikit SDKs (USB-C, NFC) | Yes, yubikit | Partly |
| YubiKey models | Every FIDO2 key from Yubico and other vendors | YubiKey 5 only | YubiKey 5 only |
| Same credential as sign-in | **Yes** | No | No |
| Setup on the key | Nothing | Generate a key in a slot, management key | Generate keys, admin PIN |

PRF is the only mechanism that works in a browser, needs no setup, works with any FIDO2 key, and
lets the sign-in credential also guard the folders. It is symmetric (the key is needed to read
**and to write** names), which fits the "touch for every action" model below; writing content
needs no key anyway, because every folder has a public age recipient.

### Which credentials can lock folders

A `user_webauthn` row is **eligible** when:

- it was created with `hmac-secret`: the key says so in the authenticator data it signs at
  registration, which webauthn-rs keeps in the stored `Credential`
  (`extensions.hmac_create_secret`, unsolicited);
- it lives on a hardware key: `backup_eligible` is false in the stored `Credential` (synced
  passkeys are backup-eligible). Either `kind` is fine: a YubiKey registered as a passkey (with a
  PIN) works as well as one registered as a second factor;
- the key has a PIN (or fingerprint): unlocking always requires user verification, because
  `hmac-secret` returns a different secret with and without it. A second-factor key without a
  PIN can sign in but cannot lock folders; the settings say "Set a PIN on this key to use it for
  protected folders".

Registration changes in the web client: `webauthn-rs-proto` 0.6.2 has no `prf` field, so
`clients/web/src/webauthn.rs` adds `extensions.prf = {}` to the creation options through
`Reflect` (done). A security key then makes the credential with `hmac-secret`, and the server
reads that from the signed data rather than from a flag the client sends, so no column is
needed: `GET /me/webauthn` reports `prf` and `synced` for each key (feature
`security_key_prf`), and `WebAuthnKey::can_protect_folders` combines them. Sign-in ceremonies
never request PRF, so a PRF value never reaches the server. Still to verify with a real
YubiKey in Chrome: that the browser sends `hmac-secret` on `makeCredential` for `prf` (the
server tests sign it with a software key).

Keys registered before this change were created without `hmac-secret` and usually cannot give a
secret. "Use for protected folders" tries one PRF assertion; when no secret comes back, the
settings ask to register the key again (remove and add, a minute's work).

### Enabling a key on the desktop

Registration stays in the web client, like sign-in. The desktop settings get "Security keys for
protected folders": it lists the account's credentials (`GET /me/webauthn`) with, for each one,
"Enabled", "Enable", or why it cannot be used (synced passkey, no PRF, no PIN). **Enable** runs a
native PRF assertion with that credential and wraps the vault key for it. With no eligible key,
the list shows "Add a security key" and opens the web security page in the browser.

### How a PRF secret is produced

1. Enabling a credential stores a random 32-byte salt with it on the server (the unlocker row).
2. To unlock, the client runs an assertion with that credential id, its stored RP id and the salt,
   using a challenge it generates itself: the server takes no part and never sees the answer. The
   user enters the PIN and touches the key. The key returns a 32-byte secret.
3. The client derives a wrapping key from it: `kek = HKDF-SHA256(secret, info="home-space vault kek v1")`.

Rules that every client has to get exactly right, or one client cannot open what another wrote:

- **The credential's own RP id.** A credential is bound to the RP id it was registered under
  (the host of the page, stored in `user_webauthn.rp_id`). Native clients pass that stored value
  as a plain string over CTAP2, so they work whatever address the app is configured with. The web
  client can use a credential only on the page of that host.
- **The PRF salt transform.** Browsers do not hand the PRF input to the key as is: they send
  `SHA-256("WebAuthn PRF" || 0x00 || input)`. Native clients that speak `hmac-secret` directly
  must apply the same transform, so the value matches the browser's.
- **Always user verification**, as above.

## Key hierarchy

```
security-key folders                                password folders

security key A ─┐                                   folder password
security key B ─┼─ each wraps ──▶ vault key              │
recovery key  ─┘                  (one per user)          ▼ Argon2id (salt, params per folder)
                                       │                  │
                                       └── wraps ──▶ folder secret ◀── wraps ──┘
                                                     (32 bytes, one per protected folder)
                                                         ├─ HKDF ─▶ name key (AES-SIV-256)
                                                         └─ HKDF ─▶ X25519 identity (age); public key on the server
                                                                           │
                                                                           └─ age recipient ─▶ file key per file version
```

- **Vault key**: random, generated on the client the first time a key is enabled. It never
  leaves a client unwrapped and is never kept: it is unwrapped for one action and wiped. Each
  unlocker (security key or recovery key) stores its own copy, wrapped with XChaCha20-Poly1305
  under its `kek`. Enabling another key wraps the vault key once more; nothing else changes.
- **Password folders have no vault.** Their folder secret is wrapped directly by
  `Argon2id(password, salt)`, so a password can never open a security-key folder, and guessing
  one folder's password gains nothing elsewhere.
- **Folder secret**: random per protected folder. A separate secret per folder lets a folder be
  re-keyed, change its lock or (later) be shared without touching the rest.
- **Name key** and **X25519 identity**: derived from the folder secret with HKDF and distinct
  `info` strings. The X25519 public key (the folder's age recipient) is stored in clear on the
  server. Content is encrypted to it without a key, so a server that swapped it could read new
  uploads: whenever a client has the folder secret (every touch), it derives the recipient and
  refuses a listed one that differs (`FolderKeys::check_recipient`).
- **File key**: age generates a random one for every file it encrypts, so every version of every
  file has its own. It is wrapped to the folder's recipient in the age header, inside the blob.
- **Change lock** (password → security key or back): unlock, re-wrap the folder secret under the
  other lock. No file is re-encrypted.

### Lost keys and recovery

Nothing is forced (decided 2026-11-26):

- The Protect dialog offers **Security key (recommended)** and **Password (less secure)**.
  - No eligible key: the security-key choice is shown but disabled, with "This folder cannot be
    protected securely without a security key. Add one in your account's security settings." and
    a button that opens them.
  - Exactly one enabled key: a warning, "If you lose this key, the files in this folder are lost.
    Add a second key or create a recovery key.", with both actions next to it. Protect still
    works.
- A **recovery key** can be created at any time: 32 random bytes shown once as a grouped Base32
  string (and a QR code) for printing. It is another unlocker of the vault, with
  `kek = HKDF(recovery bytes)`; no PRF. Being random and not on the server, it does not weaken the
  security-key lock against the server.
- **Password folders** have no recovery: a forgotten password means the data is lost, and the
  dialog says so.
- **Key management** (desktop settings, as above): enabled keys with their names, disable a key,
  create or replace the recovery key.
- **Removing a credential** on the web security page, or disabling it on the desktop, deletes its
  wrapped vault key. When it is the **last unlocker** of a vault that still has folders, the
  server refuses (`409 last-unlocker`) and the page names the folders; the user adds another key
  or a recovery key first, or unprotects or deletes the folders. Otherwise the desktop offers to
  **rotate** the vault key (`POST /vault/rotate`: re-wrap every folder secret with a new vault
  key, a few KB of work, not re-encrypting files), so someone who copied the old wrapped vault key
  and has the removed key and its PIN cannot unlock any more. The server cannot rotate on its own,
  since it never has the vault key; disabling a key on the desktop does both in one step.

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

Encrypting needs only the public recipient, so uploading content never needs the key; only its
name does.

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
- Stored in a new `sealed_name BYTEA` column (up to 1040 bytes: 255 four-byte characters,
  padded, plus the 16-byte tag). The existing `name` column holds the
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

## Unlock and lock

Decided 2026-11-26: for security-key folders the client keeps **no decryption key** between
actions. Each action that needs one asks the security key, derives what it needs, does that one
action and wipes every key (`zeroize`) before it returns.

| Action | Security-key folder | Password folder |
|---|---|---|
| **Unlock** (context menu, toolbar, or the "Locked" placeholder when the folder is opened) | PIN and touch. Decrypts the **names** of the folder's subtree, then wipes the keys. | Password. Derives the key and keeps it until Lock. |
| Browse, sort, search names | No key: the names are in memory. | Same. |
| Open, download, export | One touch for the whole selection: unwraps the age file keys of the selected files, then wipes the folder identity. | No prompt. |
| Upload, new folder, rename, move | One touch per action, to seal the names. Content is encrypted to the public recipient without a key. | No prompt. |
| **Lock**, **Lock all** | Wipes the names and the PIN token. | Wipes the names and the key. |

- **PIN once, touch each time** (decided 2026-11-26). Unlock asks for the PIN; the client keeps
  the key's CTAP2 `pinUvAuthToken` until Lock, so later actions need only a touch. The token is
  not a decryption key: it is useless without the physical key plugged in and touched, and user
  verification stays on, so `hmac-secret` gives the same secret. A CTAP 2.1 key limits how long a
  token may be used (about 10 minutes); after that, or after the key is unplugged, the next action
  asks for the PIN again. On Windows, `webauthn.dll` shows its own dialog and may ask for the PIN
  every time; the app cannot change that.
- **Viewer:** while a file is open in a viewer, its age file key (not the folder secret) stays in
  memory so a video can seek and a PDF can load pages; it is wiped when the viewer closes. The
  decrypted bytes on screen are in memory too, which cannot be avoided.
- **Password folders** keep their derived key in memory until Lock (decided 2026-11-26, for
  convenience). That is one more way the password lock is weaker, and the Protect dialog says it.
- **Lock** also revokes viewer buffers and deletes the folder's temporary decrypted files.
  Encrypting uploads in progress finish first: they need only the public recipient.
- **Auto-lock**, per folder:
  - after 15 minutes without activity in that folder (a setting);
  - on sleep and screen lock;
  - when the security key that unlocked it is unplugged (security-key folders);
  - on sign-out;
  - when the app closes.
  On a crash the names and the PIN token die with the process; temporary files are swept at the
  next start.

## Server changes

The server does no cryptography. It stores what clients give it and keeps protected data away
from features that would need plaintext.

### Schema

In `server/migrations/0032_protected_folders.sql` (done):

- `vaults (user_id, epoch, created_at)`: one per user, created with the first unlocker. The
  epoch is bumped by every rotation; unlockers and security-key folder secrets are always
  wrapped under the current one.
- `vault_unlockers (id, user_id, kind, credential, prf_salt, wrapped_vault_key, created_at)`:
  `kind` is `security_key` (with `credential → user_webauthn(id)` and a 32-byte `prf_salt`) or
  `recovery` (at most one per user). `credential` has no `ON DELETE`: removing a credential
  that is an unlocker removes the unlocker in the same transaction, or is refused when it is the
  last one of a vault that folders are locked with.
- `protected_folders (node_id, owner_id, lock, wrapped_secret, epoch, kdf, recipient, format,
  state, created_at)`: `lock` is `security_key` (with `epoch`) or `password` (with `kdf`, the
  Argon2id salt and parameters as JSON); `recipient` is checked to look like `age1` and 58
  bech32 characters; `state` is `protecting`, `protected` or `unprotecting`.
- `nodes.sealed` and `nodes.sealed_name` (48 to 1040 bytes, 16 more than a multiple of 32: the
  AES-SIV tag and the padded name). A sealed node's `name` holds the base64url SHA-256 of its
  sealed name, so the unique index on names keeps working without the server knowing them:
  sealing is deterministic for a parent, kind and name.

Wrapped keys and secrets are 73 bytes. A vault whose last unlocker goes while no folder is
locked with it is removed, so the next key starts a new vault at epoch 1.

`sealed` is set on every node below a protected folder (not on the folder itself). It is
denormalized so that every query that has to skip protected data (search, shares, listings for
VFS, changes) can filter on one column instead of walking ancestors.

### API

Done (feature `protected_folders`):

- `GET /vault`: the epoch and the unlockers, each security key with its `credential`,
  `credential_id`, `rp_id` and `prf_salt` (all useless without the key); `404` before the first.
- `POST /vault/unlockers` `{ kind, credential?, prf_salt?, wrapped_vault_key, epoch }`: enables
  a security key (which must have `prf` and not be `synced`) or sets the recovery key (replacing
  the one before). The first creates the vault and must say epoch 1; later ones the vault's
  current epoch, else `409 vault-changed`.
- `DELETE /vault/unlockers/{id}` and `DELETE /me/webauthn/{id}` of a credential that is an
  unlocker: refused with `409 last-unlocker` while it is the last unlocker and folders are
  locked with the vault. Clients should rotate after removing a key that may be lost.
- `POST /vault/rotate` `{ epoch, unlockers: [{ id, wrapped_vault_key, prf_salt? }], folders:
  [{ node_id, wrapped_secret }], remove: [id] }`: one transaction; every unlocker that stays and
  every security-key folder must be listed exactly once (`422` otherwise).
- `GET /protected`, `POST /protected` `{ node_id, lock, wrapped_secret, recipient, epoch | kdf }`
  and `PATCH /protected/{id}` `{ lock, wrapped_secret, epoch | kdf }`. Protecting refuses the
  recycle bin, Lost & Found and anything overlapping another protected folder (`409 protected`),
  and ends the sharing of the folder and everything in it. An empty folder is `protected` at
  once, one with content `protecting` until it is converted (below). Argon2id parameters must be
  a 16-byte salt, 19 MiB to 4 GiB, 1 to 16 passes and lanes.
- Everything that changes what unlocks a folder needs a recent sign-in
  (`require_recent_sign_in`): adding or removing unlockers, rotating, protecting and changing a
  lock. With a stolen access token alone, nobody can add their own unlocker, lock the user's
  files with their own key, or replace a wrapped secret with garbage.
- `GET /me/webauthn` returns `prf` and `synced` for each key (feature `security_key_prf`).

Still to do:

- `POST /folders`, `POST /files`, `POST /uploads`, `PATCH /nodes/{id}` and `POST /nodes/move` take
  `sealed_name` instead of `name` for nodes inside a protected folder. The server **rejects plaintext
  names and plaintext uploads** into a protected folder (`422 sealed_required`), so a client bug or
  an old client cannot leak data into it. It cannot check that content is encrypted, but it checks
  that it starts with the age header line.
- Clients do not put `protected_folders` in `REQUIRES`: without it they hide Protect.

### Moves

A sealed name is bound to its parent, so moving a node within a protected folder means the client
sends the newly sealed name with the move (`MoveRequest` gets an optional `sealed_names` map by
node id). Only the moved nodes change; their children keep theirs, since their parent did not
change. Merging on conflict works as today, on sealed names.

Moving **into or out of** a protected folder, or between two protected folders, is refused by the
server (`409 protection_boundary`). The client offers it as a copy that encrypts (or decrypts),
followed by deleting the source.

### Features that need plaintext

| Feature | For sealed nodes |
|---|---|
| Search (`search_entries`, content providers) | Not indexed; existing entries deleted on protect. Inside an unlocked folder the client searches the decrypted names in memory. |
| Previews, gallery, viewers | Only in the client, after decryption; a gallery of many images is one touch for the visible page. |
| Share links, shared folders, `archive` downloads | Refused (`409 protected`) for a protected folder or anything inside it. Sharing encrypted folders with other users is under [Later](#later). |
| Send to device (transfers) | Refused in phase 1. A sealed file can later be sent as ciphertext to the user's own devices. |
| Copy (`/shared/{id}/copy`, server-side copy) | Within one protected folder: allowed (copies ciphertext; the client sends the new sealed name). Elsewhere: refused. |
| Duplicates (`files/src/duplicates.rs`) | Sealed nodes are left out; random file keys make equal files differ anyway. |
| "Unchanged" upload check (`tree/rules.rs`) | Never matches for sealed files (every encryption differs); harmless, a re-upload is a new version. |
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

The server never sees plaintext after protection, but before it, it did. Protecting an existing
folder is a conversion done by the client, **in place** (decided 2026-11-26): every file keeps its
node id, every version keeps its number and dates, and only the content and names are swapped for
ciphertext. Old versions are always converted; there is no "current version only" choice.

1. The user clicks **Protect**, picks the lock and unlocks (touch, or the new password). With a
   security key and no vault yet, enabling the key runs first.
2. The client generates a folder secret and calls `POST /protected` with
   `{ node_id, lock, wrapped_secret, recipient, epoch | kdf }`. The server, in one transaction:
   - marks the folder `protecting`;
   - blocks changes to its subtree from every client (uploads, renames, moves, deletes return
     `423 locked`), so it cannot change during the conversion;
   - hides the subtree from VFS listings and sync at once;
   - deletes its search entries, share items and pending transfers.
3. The client asks `GET /protected/{id}/pending`: every node of the subtree not sealed yet,
   trashed ones included, with its versions (number, size, SHA-256). This list is the journal:
   after a crash, a closed laptop or a pause, the client simply asks again.
4. **One touch seals every name.** The client seals the names of all pending nodes at the start
   and wipes the key; encrypting content needs only the public recipient, and downloading the
   plaintext needs no key. A large tree is sealed in batches of names kept on the client until
   sent, never the key.
5. For each file, the client downloads each version (the current content and every
   `file_versions` row), encrypts it with age while streaming, and stages it with
   `POST /uploads` and `purpose: seal` (chunked as today; no node is created). Progress shows in
   the transfers panel.
6. `POST /protected/{id}/seal` with
   `{ node_id, sealed_name, versions: [{ version, source_sha256, upload_id }] }`. In one
   transaction the server:
   - checks that every version of the node is listed and that `source_sha256` matches what it
     holds (a version that changed or appeared is refused, and the client starts the node again);
   - moves each staged blob over the node blob or version blob (`store/blobs.rs`, like `commit`
     and `commit_version`), which removes the plaintext blob;
   - sets `size`, `sha256` and `mime = 'application/octet-stream'` on `nodes` and
     `file_versions`, and `sealed` and `sealed_name` on the node;
   - keeps `version`, `modified_at` and `edited_on`: the `edited_on` trigger from
     `0012_edited_on.sql` must not fire for a seal.
   A folder is sealed with `sealed_name` alone. Sealed names are bound to the parent id, which
   does not change, so the order does not matter.
7. `POST /protected/{id}/finish` checks that nothing in the subtree is left unsealed, marks the
   folder `protected` and lifts the lock.
8. The client tells the user what the server cannot erase: **server backups and snapshots,
   object storage versioning, and freed disk blocks** may still hold the old plaintext until they
   expire or are overwritten.

- **Quota:** only the versions of the file being converted are staged at a time, so the
  conversion needs at most that file's size again, not the folder's.
- **Pause and cancel:** the conversion can be paused and resumed. Cancel turns it into an
  unprotect of the nodes already sealed.
- **Unprotect** is the same in reverse: state `unprotecting`, one touch to open all names,
  every version decrypted and swapped back, then the `protected_folders` row is removed.

**Open question:** step 2 locks the subtree for the whole conversion, which can take hours for a
large folder. The alternative (allow changes, convert them in a second pass) is more work and can
wait.

## Desktop client (phase 2)

New crate `clients/vault-core` (no I/O to the server, no UI) used by desktop, web and mobile-core:

- types for the vault, unlockers and protected folders; wrapping and unwrapping (`chacha20poly1305`,
  `hkdf`, `sha2`), Argon2id for password folders (`argon2`), recovery key encoding;
- `seal_name` / `open_name` (`aes-siv`, `unicode-normalization`);
- streaming encrypt and decrypt, and range decrypt, on top of `age`;
- an `Authenticator` trait (`prf(credential, rp_id, salt)`, keeping the PIN token between calls),
  implemented by each platform; a software fake for tests;
- all secrets in `zeroize`/`secrecy` types, with the API shaped so a key lives only inside one
  call (`with_folder_secret(|s| …)`), not in a struct that outlives the action.

Desktop `Authenticator`:

- **Linux and macOS:** CTAP2 over USB HID with `hmac-secret`. Candidates: `ctap-hid-fido2` and
  `ctap-fido2` (both MIT; the latter documents `hmac-secret` and was tested on a YubiKey 5C), or
  `libfido2` through FFI. **Spike:** pick one; verify the PRF value matches Chrome's for the same
  credential and salt, that a credential registered in the browser works, and that the PIN token
  can be reused for several assertions. Linux needs the usual udev rule for FIDO devices (shipped
  by `libfido2` and most distributions). The Flatpak build needs `--device=all` for hidraw access.
- **Windows:** apps without admin rights cannot open FIDO devices directly, so the client must go
  through the Windows WebAuthn API (`webauthn.dll`, `windows` crate,
  `Win32::Networking::WindowsWebServices`), which shows the system PIN and touch prompt and
  supports `hmac-secret` for security keys. **Spike:** confirm the minimum Windows version, that
  the output matches the browser's PRF, and how often it asks for the PIN.
- Only security keys: platform authenticators and synced passkeys are not offered, even where the
  OS could reach them.

UI:

- "Protect…" in a folder's context menu (`browser/context_menu.rs`), opening the Protect dialog
  described under [Lost keys and recovery](#lost-keys-and-recovery).
- A protected folder shows a lock icon, open or closed. Opening it while locked shows "Locked"
  with an Unlock button. "Unlock" and "Lock" are in the context menu and the toolbar, and
  "Lock all" in the app menu. PIN entry is the app's own dialog on Linux and macOS, the system's
  on Windows. Every action that needs a touch says "Touch your security key" with the action's
  name, so a touch is never asked without a reason the user can see.
- Auto-lock needs two things the desktop does not have today: an inactivity timer (a new iced
  subscription in `browser/mod.rs`) and window close handling (`window::close_requests`; today
  `browser/input.rs` has none).
- **Open:** viewers (images, text, PDF, video) decrypt into memory. "Open with…" an external app
  needs a file on disk: it is decrypted into a private temporary directory (`$XDG_RUNTIME_DIR` on
  Linux, which is RAM; the app's temp dir on macOS and Windows), deleted on lock and on app exit,
  with a warning the first time. Saving back from an external app comes in phase 3 (watch the
  file, re-encrypt, upload as a new version).
- **Download** is an explicit export to a place the user picks; the app warns that the copy is
  not protected.
- **Upload** into an unlocked protected folder (drag and drop, file picker) asks one touch to seal
  the names, then encrypts while uploading.

## Web client

Chrome can reach the key, decrypt in memory and hand the files out:

- **Key access:** `navigator.credentials.get` with the `prf` extension and the stored credential
  id. Works with security keys in Chrome and Edge (since 116), and in recent Firefox. Safari
  supports PRF for its own passkeys; for USB and NFC security keys it had a bug returning
  unusable output as of early 2026, so Safari is treated as unsupported until verified. The
  credential works only on the host it was registered on (its RP id).
- **Prompts:** the browser asks for the PIN on every assertion, so on the web each action is PIN
  and touch.
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
Phase 4, after desktop.

## Mobile (phase 5)

- **Android:** `yubikit-android` (FIDO2 over USB-C and NFC, with `hmac-secret`) through mobile-core.
  Using the raw CTAP2 session instead of Android Credential Manager avoids needing a Digital
  Asset Links file on every self-hosted server. Implemented in Kotlin, behind the same
  `Authenticator` trait exposed through UniFFI. Over NFC every action is a tap of the key on the
  phone.
- **iOS:** Yubico's `yubikit-ios` (NFC, Lightning, USB-C). `ASAuthorization` security key APIs need
  an associated domain for the RP id, which cannot be listed at build time for self-hosted
  servers, so the raw CTAP2 path is the only one that works. **Spike** needed.
- Password folders work on both without extra hardware.
- Files open in the in-app viewers only; sharing a decrypted file to another app writes a
  temporary copy that is deleted when the share sheet closes.
- The mobile file providers never show protected folders (same `sealed=include` rule).

## Phases

1. **Foundation** (server + `vault-core`): the PRF registration change in the web client and
   `prf` / `synced` in the key list (done); `vault-core` (done); schema, vault and protected folder API, `409 last-unlocker` (done); sealed names,
   refusing plaintext, hiding from search, shares, transfers, VFS and sync; in-place conversion
   endpoints; `vault-core` with tests against fixed vectors, including Argon2id; Authenticator
   spikes for Linux, macOS and Windows.
2. **Desktop**: enabling keys, recovery key, the Protect dialog with both locks, conversion of new
   and existing folders with all versions, Unlock / Lock / Lock all, auto-lock, browse, open,
   upload, download, move and rename inside, recycle bin.
3. **Desktop polish**: external-app save-back, change lock, unprotect.
4. **Web** (opt-in), with Chrome, Edge and Firefox in the e2e tests (a virtual authenticator with
   PRF in Chrome DevTools Protocol / Playwright).
5. **Android**, then **iOS**.

## Testing

- `vault-core`: fixed test vectors for the KEK derivation, PRF salt transform, Argon2id, name
  sealing and age files; property tests that seal/open and encrypt/decrypt round-trip, including
  range reads; a test that no key type outlives the call that made it.
- The software authenticator of the server tests (`server/crates/api/tests/webauthn/authenticator.rs`)
  gains `hmac-secret` and `prf` results, and a `vault-core` fake `Authenticator` uses the same
  keys, so the e2e tests (`e2e/`) can drive the whole flow without hardware.
- Cross-client check: a key registered in Chrome locks a folder on the desktop, and a file
  protected by the desktop opens in Chrome with the same key (manual, with a real YubiKey, before
  each phase ships).
- Server tests: plaintext rejected inside protected folders; sealed nodes absent from search,
  shares, VFS listings and changes without `sealed=include`; the seal swap keeps node ids, version
  numbers and dates, refuses a stale `source_sha256`, and leaves no plaintext blob or search entry
  behind; `423` on the subtree while protecting; `409 last-unlocker` and rotation when a
  credential is removed.

## Later

- **Write without a touch:** the folder's X25519 recipient is public, so a client could encrypt
  new files into a protected folder without the key (for example mobile auto-upload into a
  protected "Scans" folder). Names would be sealed to the recipient instead of with AES-SIV and
  re-sealed on the next unlock.
- **Sharing a protected folder with another user:** wrap the folder secret to that user's vault
  public key.
- **Rollback detection:** a signed, versioned manifest per folder so a client notices when the
  server withholds or rolls back files.
- **Seal the root name** (see open questions).
- **Synced passkeys** (Google Password Manager, iCloud Keychain, Windows Hello) as unlockers in
  the web client: PRF works with them, but they are not hardware keys, native apps cannot reach
  them, and losing the account loses the passkey.

## Open questions

1. Root folder name: stays in clear (proposed), or sealed too?
2. Auto-lock timeout default: 15 minutes (proposed)?
3. Web client: opt-in per vault (proposed), or not at all?
4. Protecting an existing folder: lock the subtree during conversion (proposed), or allow changes
   and do a second pass?
5. Argon2id parameters for password folders: 64 MiB, 3 passes, 1 lane (proposed, so the web and
   phones can run it in about a second), and a 12-character minimum?
6. Password folders: offer a recovery key too, or keep "forgotten password, lost data"
   (proposed)?
7. Minimum hardware: any FIDO2 key with `hmac-secret` and a PIN (proposed), or YubiKey only?
