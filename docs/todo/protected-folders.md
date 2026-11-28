# Protected folders: end to end, with a security key or a password

## Problem

Everything a user stores is readable by whoever runs, backs up or breaks into the server: file
content on disk (or in object storage, see `object-storage.md`), names in `nodes`, and extracted
text in `search_entries`. Some folders (identity documents, tax papers, medical records, keys)
should be readable only on the user's own devices.

A folder's context menu offers **Protect with a security key** and **Protect with a password**:

- **Security key** (recommended): one of the user's folder keys, security keys (a YubiKey or any
  FIDO2 key) enabled for the vault (see [Folder keys](#folder-keys-credentials-of-their-own)).
  The secret is on the key, behind its PIN.
- **Password**: a password chosen for the folder. The client derives a key from it (Argon2id)
  and nothing about the password is on the server.

Both are **end to end** (decided 2026-11-27): the client encrypts names and content, and someone
with the server and all its data still cannot decrypt the folder without the key or the
password. Both add **Unlock** and **Lock** to the folder, and both lock themselves after
inactivity and when the app closes. Web and desktop first, mobile later; the web client carries a
trust caveat ([Web client](#web-client)).

## The two locks

Decided 2026-11-26, changed 2026-11-27: password folders were first encrypted by the server, with
a key it kept. They are end to end now, like security-key folders, and the server no longer
encrypts, unlocks or converts anything.

| | Security key | Password |
|---|---|---|
| Who encrypts | The client (`shared/vault-core`) | The client (`shared/vault-core`) |
| What the server can read | Nothing: names and content are ciphertext it stores like any other bytes | The same |
| Key | Folder secret wrapped by the user's vault, which each folder key (and an optional recovery key) unwraps through WebAuthn PRF / `hmac-secret` | Folder secret wrapped by a key derived from the password (Argon2id, parameters stored with it), and again by the folder's own recovery key |
| Unlock | PIN and touch, once | The password, once; checked by the client (a wrong one does not unwrap) |
| While unlocked | The folder's keys stay in the client's memory until Lock, inactivity, or the key is unplugged | The same, until Lock or inactivity |
| Protecting an existing folder | The client downloads the files, deletes them from the server (for good, see below), and uploads them encrypted. **Their version history is lost**, and the user is warned first. | The same |
| Names | Sealed by the client (AES-SIV), stored as base64url text in the normal `name` | The same |
| Old clients and on-demand files | See meaningless names (base64url); new clients hide protected folders there | The same |
| Deleting | **For good**, versions included: nothing inside goes to the recycle bin | The same |
| Weakest point | The key and its PIN | The password: whoever has what the server keeps can guess passwords offline |

## Threat model

Protects against:

- Someone who reads the server's disk, database, object storage bucket or backups, a server
  operator, or an attacker with the server who reads data passively:
  - **Security-key folders:** fully. What the server holds (credential ids, salts, wrapped keys,
    ciphertext) is useless without the physical key and its PIN.
  - **Password folders:** as far as the password is strong. What the server holds (Argon2id
    parameters, the secret wrapped by the password's key and by the recovery key, ciphertext)
    lets someone guess passwords offline, each guess costing 64 MiB and three Argon2id passes,
    with no throttling. Clients ask for a password the account's rules accept and say so.
- Someone who steals a laptop with an app signed in, or an access token: a security-key folder
  cannot be opened without the key and its PIN, a password folder not without its password
  (though a stolen token can fetch what is needed to guess it offline). Everything that changes
  what unlocks a folder needs a recent sign-in.
- Someone who steals the security key: without its PIN the key does not answer (FIDO2 user
  verification is required, and the key locks itself after 8 wrong PINs).

Does **not** protect against:

- An attacker controlling a client while the user works in it (malware on the desktop, a
  malicious browser extension). While a folder is unlocked its keys are in the app's memory.
- A server that serves a **modified web client**. This only affects the web client; see
  [Web client](#web-client).
- Metadata. The server still sees: which folder is protected and with which lock, how many files
  and folders it has and how they nest, file sizes (rounded only by the 64 KiB chunks plus a
  small header), how many versions each file has, when files are created and changed, which
  device changed them, and that names within one folder differ. It does not see names, MIME
  types or content.
- Deletion. The server, or someone with a recent sign-in, can delete or roll back ciphertext, or
  replace a wrapped secret so that nothing opens the folder. Tampering is detected (AEAD), but
  the server can withhold a file or serve an older version of it. Rollback detection is listed
  under [Later](#later).
- Plaintext from before protection that the server cannot erase: backups, snapshots, object
  storage versioning and freed disk blocks.

## Folder keys: credentials of their own

Decided 2026-11-27: a security key unlocks protected folders through a **folder key**, a WebAuthn
credential the client makes on the key for the vault alone, per user. It is never a sign-in
credential (`user_webauthn`, `local-accounts.md`), though it may live on the same physical key
as one, under another credential id. So removing a sign-in key, or an administrator's reset of
the second factors (as when an account moves from single sign-on to a password), never touches
the vault, and the vault never keeps a sign-in key from being removed.

### Why PRF and not RSA

There are three ways a YubiKey can protect a key:

| | FIDO2 `hmac-secret` / WebAuthn PRF | PIV (RSA / ECC, slot 9d) | OpenPGP card |
|---|---|---|---|
| Browser (Chrome, Edge, Firefox) | **Yes**, WebAuthn PRF | No | No |
| Desktop (Linux, macOS, Windows) | Yes: CTAP2 over HID; on Windows through `webauthn.dll` | Yes, PC/SC | Yes, PC/SC or gpg-agent |
| Android, iOS | Yes, Yubico's yubikit SDKs (USB-C, NFC) | Yes, yubikit | Partly |
| YubiKey models | Every FIDO2 key from Yubico and other vendors | YubiKey 5 only | YubiKey 5 only |
| Same physical key as sign-in | **Yes** | Yes, another applet | Yes, another applet |
| Setup on the key | Nothing | Generate a key in a slot, management key | Generate keys, admin PIN |

PRF is the only mechanism that works in a browser, needs no setup and works with any FIDO2 key,
including the one the user signs in with. It is symmetric (the key is needed to read
**and to write** names), which fits the "touch for every action" model below; writing content
needs no key anyway, because every folder has a public age recipient.

### What a folder key is

- A credential made by an app with CTAP2 `makeCredential` (the web client's
  `navigator.credentials.create` ceremony predates the decision to leave keys to the apps) with a challenge the client makes itself: the server takes no part in its ceremonies
  and never checks an attestation or an assertion. Not discoverable (`residentKey:
  discouraged`), always with user verification, and with the `prf` extension (`hmac-secret`),
  asked for the new salt at once. The user handle is random, never the account's, so a key that
  keeps the credential anyway cannot replace a sign-in passkey with it.
- Only on a hardware key: a new credential whose authenticator data says it is backed up (a
  synced passkey) is refused, since the native apps cannot reach those.
- The key needs a PIN (or fingerprint): `hmac-secret` gives a different secret with and without
  user verification, so unlocking always asks for it.
- The server keeps it as an unlocker (`vault_unlockers`): its name (at most 64 characters, like
  a sign-in key's), credential id, RP id, PRF salt and wrapped vault key. The RP id must be one
  of the tenant's names, the host of its `public_url` or one of its domains, or, for a tenant
  with neither, the host the request was sent to (`browser::WebAuthnSites`, as for passkeys).

Sign-in ceremonies never ask for PRF, so a PRF value never reaches the server, and
`GET /me/webauthn` says nothing about it.

### Enabling a key

Decided 2026-11-27: security keys are for the desktop and mobile apps only; the web client never
holds the vault key (see [Web client](#web-client)).

Users see folder keys as **encryption keys** (renamed 2026-11-28 in both clients), beside their
**sign-in keys**; the code keeps calling them folder keys.

- **Web:** the security page lists **Sign-in keys** and **Encryption keys** apart (done), and
  removes one (done), which needs no key material. It never adds one: enabling is in the apps.
- **Desktop:** Settings → Account → "Encryption keys" lists them (`GET /vault`).
  **Enable** makes a new credential with CTAP2 `makeCredential` for the server's host, with
  `hmac-secret`, and wraps the vault key for it. The vault key is new for the first key; for
  another, an existing folder key or the recovery key opens the vault first.

### How a PRF secret is produced

1. Enabling a folder key stores a random 32-byte salt with it on the server (the unlocker row).
2. To unlock, the client runs an assertion with that credential id, its stored RP id and the salt,
   using a challenge it generates itself: the server takes no part and never sees the answer. The
   user enters the PIN and touches the key. The key returns a 32-byte secret.
3. The client derives a wrapping key from it: `kek = HKDF-SHA256(secret, info="home-space vault kek v1")`.

Rules that every client has to get exactly right, or one client cannot open what another wrote:

- **The credential's own RP id.** A credential is bound to the RP id it was made for (the host
  of the page, stored in `vault_unlockers.rp_id`). Native clients pass that stored value
  as a plain string over CTAP2, so they work whatever address the app is configured with. The web
  client can use a credential only on the page of that host.
- **The PRF salt transform.** Browsers do not hand the PRF input to the key as is: they send
  `SHA-256("WebAuthn PRF" || 0x00 || input)`. Native clients that speak `hmac-secret` directly
  must apply the same transform, so the value matches the browser's.
- **Always user verification**, as above.

## Security-key folders

### Key hierarchy

```
security key A ─┐
security key B ─┼─ each wraps ──▶ vault key (one per user)
recovery key  ─┘                      │
                                      └── wraps ──▶ folder secret (32 bytes, one per folder)
                                                        ├─ HKDF ─▶ name key (AES-SIV-256)
                                                        └─ HKDF ─▶ X25519 identity (age); public key on the server
                                                                       │
                                                                       └─ age recipient ─▶ file key per file version
```

- **Vault key**: random, generated on the client the first time a key is enabled. It never
  leaves a client unwrapped and is never kept: it is unwrapped for one action and wiped. Each
  unlocker (security key or recovery key) stores its own copy, wrapped with XChaCha20-Poly1305
  under its `kek`. Enabling another key wraps the vault key once more; nothing else changes.
- **Folder secret**: random per protected folder. A separate secret per folder lets a folder be
  re-keyed or (later) shared without touching the rest.
- **Name key** and **X25519 identity**: derived from the folder secret with HKDF and distinct
  `info` strings. The X25519 public key (the folder's age recipient) is stored in clear on the
  server. Content is encrypted to it without a key, so a server that swapped it could read new
  uploads: whenever a client has the folder secret (every unlock), it derives the recipient and
  refuses a listed one that differs (`FolderKeys::check_recipient`).
- **File key**: age generates a random one for every file it encrypts, so every version of every
  file has its own. It is wrapped to the folder's recipient in the age header, inside the blob.

### Lost keys and recovery

Nothing is forced (decided 2026-11-26):

- The Protect dialog of the apps offers **Security key (recommended)** and **Password**; the web
  client offers Password only.
  - No folder key: the security-key choice is shown but disabled, with "This folder cannot be
    protected securely without a security key. Add one in your account's security settings." and
    a button that opens them.
  - Exactly one folder key: a warning, "If you lose this key, the files in this folder are lost.
    Add a second key or create a recovery key.", with both actions next to it. Protect still
    works.
- A **recovery key** can be created at any time: 32 random bytes shown once as a grouped Base32
  string (and a QR code) for printing. It is another unlocker of the vault, with
  `kek = HKDF(recovery bytes)`; no PRF. Being random and not on the server, it does not weaken the
  security-key lock against the server.
- **Key management** (web security page and desktop settings, as above): folder keys with their
  names, remove a key, create or replace the recovery key.
- **Removing a folder key** deletes its wrapped vault key. Removing a sign-in key, or an
  administrator's reset of the second factors, never does: folder keys are not sign-in keys. When it is the **last unlocker** of a vault that still has folders, the
  server refuses (`409 last-unlocker`) and the page names the folders; the user adds another key
  or a recovery key first, or unprotects or deletes the folders. Otherwise the desktop offers to
  **rotate** the vault key (`POST /vault/rotate`: re-wrap every folder secret with a new vault
  key, a few KB of work, not re-encrypting files), so someone who copied the old wrapped vault key
  and has the removed key and its PIN cannot unlock any more. The server cannot rotate on its own,
  since it never has the vault key; disabling a key on the desktop does both in one step.

### Content: age

Each file is an age v1 file with one X25519 recipient, the folder's:

- Header: the file key wrapped to the folder recipient, plus a MAC. ~200 bytes.
- Payload: ChaCha20-Poly1305 in 64 KiB chunks with a counter nonce and a last-chunk flag, so
  truncation, reordering and chunk swapping are detected. A chunk can be decrypted on its own,
  which allows range reads (video seeking in the viewer) without downloading the whole file.

Why age rather than a home-grown AES-GCM format: the format is specified, has two independent
implementations (`age`/`rage` in Rust, `age` in Go), and is already chunked and seekable. The
`age` crate builds for WASM. ChaCha20-Poly1305 is constant-time in software, which matters in
WASM and on phones without AES instructions. It also means a user can decrypt an exported file
with the standard `age` tool once they export the folder's identity (an emergency "export keys"
action, behind an unlock).

Encrypting needs only the public recipient, so uploading content never needs the key; only its
name does. Uploads are sent as `application/octet-stream`; the client derives the type from the
decrypted name.

### Names: deterministic AES-SIV as base64url text

The server enforces `UNIQUE (owner_id, parent_id, kind, name)` and the clients rely on 409
conflicts for "keep both / override / skip". AES-SIV (RFC 5297, `aes-siv` crate) is
deterministic: the same name in the same folder gives the same ciphertext, so the existing
uniqueness check keeps working without the server knowing the names.

- `name = base64url(AES-SIV(name key, ad = [parent_id, kind], pad(NFC(name))))`, without padding
  characters. With the parent id as associated data, the same name in two folders gives two
  unrelated ciphertexts.
- The plaintext is padded with `0x80` and zeros to a multiple of 32 bytes, so the length of a name
  is hidden to that granularity.
- **The server stores it as the node's name like any other** (decided 2026-11-26): no extra
  column, no check that it is encrypted. Base64url holds neither `/` nor `\`.
- **Names are as long as anywhere else** (decided 2026-11-27): up to 255 characters after NFC,
  which the client checks before sealing. The longest, 255 four-byte characters, pads to 1024
  bytes and seals to 1040 with the tag, 1387 characters of base64url. The server allows names up
  to 1387 characters inside protected folders only (`MAX_SEALED_NODE_NAME_CHARS`); everywhere
  else the limit stays 255.
- **Case:** names compare by bytes, as today. Unicode is normalized to NFC before sealing, so
  macOS (NFD) and the others agree.

**The protected folder's own name** stays in clear, so the user sees "Taxes 🔒" in the listing
without the key. Everything inside it is sealed. **Open question:** offer to seal the root name
too.

### The server treats them like any other folder

Decided 2026-11-26: the server does no cryptography for security-key folders and does not tell
their nodes apart. It is the client's job to encrypt; the server stores and serves bytes. Someone
who lists such a folder without the key (an old client, the web client, the API)
sees names with no meaning and can count the files, nothing more.

What the server does:

- Keeps the vault, the unlockers and the `protected_folders` rows (see [Server](#server)).
- Ends the sharing of the folder and everything in it when it is protected.

What follows for the other features, without special cases on the server:

| Feature | In a security-key folder |
|---|---|
| Search | The server indexes nothing inside a protected folder of either lock. Inside an unlocked folder the client searches the decrypted names in memory. |
| Previews, gallery, viewers | Only in the client, after decryption; no prompt once the folder is unlocked. |
| Shares, transfers, archive downloads | Refused for a protected folder and what it holds (`409 protected`), for both locks, since they would hand out ciphertext. A shared folder or a folder sent to a device that holds a protected folder leaves it out, in listings, archives and copies. Sharing encrypted folders with other users is under [Later](#later). |
| Moves and copies | Within the folder, moves work as today; the client sends the name sealed for the new parent. Into or out of it, the server refuses (`409 protected`): the client copies with encryption (or decryption) and deletes the source, since a plain move would leave plaintext inside or ciphertext outside. |
| Versions | Kept from the moment of protection on. Each version is its own age file with its own file key. |
| Recycle bin | Not used: deleting inside deletes for good (see [Deleting](#deleting-inside-a-protected-folder)). |
| Duplicates, "unchanged" upload check | Never match: random file keys make equal files differ. |
| Storage quota | Counts ciphertext size, which is slightly larger. |
| On-demand files, folder sync, mobile auto-upload | The clients hide protected folders there (they know them from `GET /protected`), so the system file browser never shows meaningless names. A sync mapping cannot point at or contain one. |

### Protecting an existing folder

Decided 2026-11-26: the server does not rewrite content in place. The client replaces the folder's
content, and **the files' version history is lost**. The Protect dialog says so before anything
happens: "The previous versions of the files in this folder will be deleted."

1. The user clicks **Protect** and touches the key (enabling it first if there is no vault yet).
   The client generates a folder secret and calls `POST /protected`. The folder has content, so
   it is `protecting`: other devices show it as being protected and do not write into it.
2. The client downloads the current content of every file into a private temporary directory
   (`$XDG_RUNTIME_DIR` on Linux, the app's temp dir on macOS and Windows), keeping a journal there
   of what it has done, so it can resume after a crash or a closed laptop.
3. It deletes the folder's content on the server (`DELETE /nodes/{id}` of each child). The folder
   is protected already, so they are deleted for good, old versions included, without passing
   through the recycle bin. It also offers to purge files of this folder that were in the bin
   before (their `original_path` is inside it).
4. With the new folder's keys it seals every name, then encrypts each file to the folder's recipient and
   uploads it, recreating the folders. Progress shows in the transfers panel.
5. `POST /protected/{id}/finish` marks the folder `protected`. The temporary directory is deleted.
6. The client tells the user what the server cannot erase: **server backups and snapshots,
   object storage versioning, and freed disk blocks** may still hold the old plaintext until they
   expire or are overwritten.

- **Cancel** before step 3: `DELETE /protected/{id}` (allowed while `protecting`). After it, the
  client finishes uploading what it removed, encrypted, since that is the only copy apart from its
  temporary directory.
- **Quota:** the files leave the server before their encrypted copies arrive, so the conversion
  needs little more space than the folder takes.
- **Old clients** can still write into a `protecting` folder; what they write in the meantime is
  plaintext. The client checks the folder against its journal before `finish` and encrypts
  anything new it finds.
- **Unprotect** is the same in reverse: `POST /protected/{id}/unprotect` (`unprotecting`),
  download and decrypt, delete (for good), upload plain, then `DELETE /protected/{id}`. A
  `protected` folder's row is never deleted directly, since its secret would go with it.

## Password folders

Decided 2026-11-27: a password folder is end to end like a security-key folder; only the way to
its secret differs. Everything under [Security-key folders](#security-key-folders) about content
(age), sealed names, their 255-character limit, protecting an existing folder and how the server
treats the folder holds for password folders too.

### Key

```
password ── Argon2id(NFC(password), salt, 64 MiB, 3 passes, 1 lane) ── HKDF ─▶ kek ─┐
recovery key (per folder) ─────────────── HKDF ─▶ recovery kek ────────────────────┼─ each wraps ─▶ folder secret
                                                                                    ┘
```

- **Folder secret**: random, made by the client when it protects the folder, exactly as for a
  security-key folder (name key and age identity derived from it).
- **Password KEK**: `HKDF-SHA256(Argon2id(NFC(password), salt), "home-space password kek v1")`.
  The salt (16 bytes) is new for every password; the costs are 64 MiB, 3 passes and 1 lane (the
  browser derives on a single thread), about a second. Clients refuse parameters a server lists
  outside 19 to 256 MiB, 2 to 10 passes and 1 to 4 lanes, so a server can neither make a phone
  derive for minutes nor offer a cheap derivation; the server checks the same bounds.
- **Recovery key**: 32 random bytes per folder, printed like the vault's (55 characters in 11
  groups), shown once after protecting. `kek = HKDF-SHA256(recovery, "home-space folder recovery
  kek v1")`, distinct from the vault's recovery KEK.
- Wrapping is the same XChaCha20-Poly1305 with the folder's node id as associated data as for a
  security-key folder's secret (73 bytes).
- Known-answer vectors for both KEKs are in `shared/vault-core/src/vectors.rs`.

### What the server does

- `POST /protected` `{ node_id, lock: "password", wrapped_secret, recipient, kdf: { salt,
  memory_kib, iterations, parallelism }, recovery_wrapped_secret }` (recent sign-in) stores them.
  It never sees the password or a key, and cannot tell a right password from a wrong one.
- `GET /protected` lists them back, for a client to unlock with.
- **Change the password**: the client opens the secret with the current password, or with the
  recovery key when the password is forgotten, wraps it for the new password with a new salt,
  and sends `POST /protected/{id}/password` `{ wrapped_secret, kdf, recovery_wrapped_secret? }`
  (recent sign-in). With the recovery key, it also makes a new recovery key, shown once, and
  sends the secret wrapped by it: a recovery key that was typed in is replaced. The secret stays,
  so nothing is re-encrypted. The server cannot check that the client knew the old password;
  anyone with a recent sign-in can replace the wrap, which destroys access like deleting the
  files would. The folder's recipient never changes, and clients check it against every secret
  they unwrap, so a replaced wrap cannot redirect new uploads.
- `finish`, `unprotect` and `DELETE /protected/{id}` work as for security-key folders.
- Without the password and the recovery key, the files are lost.

## Deleting inside a protected folder

Decided 2026-11-26: deleting a file or folder inside a protected folder, of either lock, deletes
it **for good**, with its old versions; nothing goes to the recycle bin. A binned file would keep
its encrypted content and sealed name outside the folder, where nothing opens them (a sealed name
is bound to its folder), restoring it into Lost & Found would need decrypting it there, and
deleting the protected folder would take along the key its binned files need.

- Deleting a folder that holds protected folders bins its other files as usual and deletes what
  the protected folders hold for good.
- A move whose `on_conflict: skip` would bin a file inside a protected folder is rolled back with
  `409 protected`.
- Clients say "Deleted for good" before deleting inside a protected folder, or a folder that
  holds one.

## Unlock and lock

Decided 2026-11-27: an unlocked folder is its **keys in the client's memory** (`FolderKeys`,
wiped on drop), whatever its lock. Unlocking asks once; then every action works without a prompt
until the folder locks. (Before, a security-key folder asked for a touch for every action and
kept no key; that is dropped as too tedious for what it protected against: malware that reads the
app's memory can also read what the user opens.)

| Action | Security-key folder | Password folder |
|---|---|---|
| **Unlock** (context menu, toolbar, or the "Locked" placeholder when the folder is opened) | PIN and touch. Unwraps the vault key, then the folder secret; keeps the folder's keys and drops the rest. | The password. Derives the KEK and unwraps the folder secret; a wrong password does not unwrap. Keeps the folder's keys. |
| Browse, sort, search names | Names opened in memory | The same |
| Open, download, export, upload, new folder, rename, move | No prompt: the keys are in memory | The same |
| **Lock**, **Lock all** | Wipes the keys and the names | The same |

- **Auto-lock**, per folder:
  - after **15 minutes without activity** in that folder (opening, uploading, renaming,
    browsing it): `AUTO_LOCK_AFTER` in `client_core::protected`, one constant for every client
    (decided 2026-11-27);
  - when the security key that unlocked it is unplugged, where the platform can tell (desktop
    over USB HID, Android over USB; not the web, and not NFC, where the key is only there for
    the tap);
  - on sign-out;
  - when the app closes, or the page is closed or reloaded.
  On a crash the keys die with the process; temporary files are swept at the next start.
- **Viewer:** the decrypted bytes on screen are in memory, which cannot be avoided. Lock closes
  the viewers of the folder, revokes their buffers (`Blob` URLs on the web) and deletes its
  temporary decrypted files. Encrypting uploads in progress finish first: they need only the
  public recipient.

## Server

### Schema

In `server/migrations/0032_protected_folders.sql` (done):

- `vaults (user_id, epoch, created_at)`: one per user, created with the first unlocker. The
  epoch is bumped by every rotation; unlockers and security-key folder secrets are always
  wrapped under the current one. A vault whose last unlocker goes while no folder is locked with
  it is removed, so the next key starts a new vault at epoch 1.
- `vault_unlockers (id, user_id, kind, name, credential_id, rp_id, prf_salt, wrapped_vault_key,
  created_at)`: `kind` is `security_key`, a folder key (with its name, credential id, RP id and
  a 32-byte `prf_salt`, the credential id unique per user), or `recovery` (at most one per
  user). Nothing refers to `user_webauthn`.
- `protected_folders (node_id, owner_id, lock, recipient, wrapped_secret, epoch,
  recovery_wrapped_secret, kdf_salt, kdf_memory_kib, kdf_iterations, kdf_parallelism, format,
  state, created_at)` (as `0034_password_folders_end_to_end.sql` recreated it): every folder has
  `wrapped_secret` and `recipient` (checked to look like `age1` and 58 bech32 characters); a
  security-key folder has `epoch`, a password folder the recovery copy and the Argon2id
  parameters. `state` is `protecting`, `protected` or `unprotecting`. Wrapped keys and secrets
  are 73 bytes.

Nothing on `nodes` changes: names and content of protected folders are stored as they come.

### API

Done (feature `protected_folders`):

- `GET /vault`: the epoch and the unlockers, each folder key with its `name`, `credential_id`,
  `rp_id` and `prf_salt` (what a client needs to ask the key, useless without it); `404`
  before the first.
- `POST /vault/unlockers` `{ kind, name?, credential_id?, rp_id?, prf_salt?, wrapped_vault_key,
  epoch }`: enables a folder key (`name`, `credential_id`, `rp_id` of a host this server
  answers on, `prf_salt`; `422` for one already enabled) or sets the recovery key (none of them;
  it replaces the one before). The first creates the vault and must say epoch 1; later ones the
  vault's current epoch, else `409 vault-changed`.
- `DELETE /vault/unlockers/{id}`: refused with `409 last-unlocker` while it is the last unlocker
  and folders are locked with the vault. Clients should rotate after removing a key that may be
  lost. `DELETE /me/webauthn/{id}` and the administrator's reset of second factors leave the
  vault alone.
- `POST /vault/rotate` `{ epoch, unlockers: [{ id, wrapped_vault_key, prf_salt? }], folders:
  [{ node_id, wrapped_secret }], remove: [id] }`: one transaction; every unlocker that stays and
  every security-key folder must be listed exactly once (`422` otherwise).
- `GET /protected`; `POST /protected` `{ node_id, lock: "security_key", wrapped_secret,
  recipient, epoch }` or `{ node_id, lock: "password", wrapped_secret, recipient, kdf,
  recovery_wrapped_secret }`. Protecting refuses the recycle bin, Lost & Found and anything overlapping
  another protected folder (`409 protected`), and ends the sharing of the folder and everything in
  it. An empty folder is `protected` at once, one with content `protecting`.
- `POST /protected/{id}/finish` (`protecting` → `protected`), `POST /protected/{id}/unprotect`
  (`protected` → `unprotecting`) and `DELETE /protected/{id}` (only while `protecting` or
  `unprotecting`).
- Everything that changes what unlocks a folder needs a recent sign-in
  (`require_recent_sign_in`): adding or removing unlockers, rotating, protecting, unprotecting
  and removing a protection. With a stolen access token alone, nobody can add their own unlocker,
  lock the user's files with their own key, or throw away a folder's secret.
- `POST /protected/{id}/password` `{ wrapped_secret, kdf, recovery_wrapped_secret? }`: a
  password folder's secret wrapped for a new password (see [Password folders](#password-folders)).
- `409 protected` for: moving a node into or out of a protected folder (either lock: inside is
  ciphertext, outside plain; the folder itself moves); sharing or sending anything in one; saving
  a transfer or copying shared items into one, which would land unencrypted; and a resumable
  upload into one with `keep_both`, since the server would number the client's sealed name and
  it would no longer open. Clients name such a file anew themselves.
- Clients do not put `protected_folders` in `REQUIRES`: without it they hide Protect.

## Desktop client (phase 2)

`shared/vault-core` (done: no I/O, no UI) is used by desktop, web and mobile-core; the server
does not use it:

- wrapping and unwrapping the vault key and folder secrets (`chacha20poly1305`, `hkdf`, `sha2`),
  recovery key encoding, and a password folder's Argon2id KEK (`argon2`);
- `seal_name` / `open_name` (`aes-siv`, `unicode-normalization`), as base64url text;
- streaming encrypt and decrypt, and seekable decrypt, on top of `age`;
- an `Authenticator` trait (`prf(credential, salt)`), implemented by each platform, and a
  software one for tests;
- every secret in a `zeroize` type; the vault key lives only inside one call
  (`with_vault_key(|key| …)`), and an unlocked folder is its `FolderKeys`, held until Lock
  (`vault::unlock_folder`, `password::unlock_with_password`).

Desktop `Authenticator`:

- **Linux and macOS** (decided 2026-11-28): `ctap-hid-fido2` (MIT), CTAP2 over USB HID with
  `hmac-secret`, in `desktop/src/protected/security_key.rs`. Chosen over `ctap-fido2` (too
  new and little used) and 1Password's `passkey-rs` (its USB client sends neither the PIN nor
  extensions). Every request sends the PIN, so user verification is always on and the output is
  the one a browser's PRF with `userVerification: required` gets. The PIN protocol is the first
  one the key lists (1 or 2). The library reports CTAP statuses only inside its error texts
  (`response_status err = 0x31 …`), which the app reads to tell a wrong PIN from a timeout; a
  change of wording there turns those messages into "did not answer". Tried on a YubiKey 5
  (FIDO_2_0 firmware, PIN protocol 1 only): it lists the key, and a probe without user presence
  answers `no credentials` without a touch. Linux needs the usual udev rule for FIDO devices
  (shipped by `libfido2` and most distributions); building needs libudev's headers
  (`libudev-dev`, `systemd-devel`), since hidapi compiles against them. The Flatpak has
  `--device=all` for hidraw. Not checked: that the output matches Chrome's PRF for the same
  credential, now that no client uses security keys in a browser; PIN protocol 2 on a real key;
  macOS.
- **Windows:** apps without admin rights cannot open FIDO devices directly, so the client must go
  through the Windows WebAuthn API (`webauthn.dll`, `windows` crate,
  `Win32::Networking::WindowsWebServices`), which shows the system PIN and touch prompt and
  supports `hmac-secret` for security keys. **Spike:** confirm the minimum Windows version, that
  the output matches the browser's PRF, and how often it asks for the PIN.
- Only security keys: platform authenticators and synced passkeys are not offered, even where the
  OS could reach them.

UI:

**Password folders on the desktop (done 2026-11-28)**, in `clients/desktop/src/protected/` (the
folder list, unlocked keys, encryption on disk, the conversion) and `browser/protect.rs` (the
dialogs):

- **Protect with password…**, **Unlock…**, **Lock**, **Change password…** (with "Forgot the
  password?" and the recovery key), **Unprotect…** and **Finish protecting / unprotecting** are
  in a folder's context menu. The account menu has **Lock all**; a banner inside an unlocked
  folder has Lock and Lock all. Deriving the key runs off the interface's thread. Protecting,
  changing the password and unprotecting confirm it is the user when the server asks (account
  password or authenticator code); a passkey-only account is told to sign in again.
- A protected folder shows a lock icon, open or closed. Opening it while locked shows "“Name” is
  locked" with Unlock; a folder inside a locked one goes to it.
- **Auto-lock:** a subscription checks every 15 s (`LOCK_CHECK`); browsing, opening, the viewer and
  uploads count as activity. Closing the window (`exit_on_close_request: false`, then
  `window::close_requests`) and signing out lock everything and delete the private folder.
- **Private folder:** `$XDG_RUNTIME_DIR/home-space-protected` on Linux, the temp dir elsewhere,
  `0700`. It holds the encrypted copies of uploads and the plain and encrypted copies a conversion
  passes through, each deleted at once, and the decrypted copies opened with other apps, deleted
  on lock. It is wiped at start, on sign-out and on close.
- **Listings and names:** opened with the keys, with the file type from the name. In a protected
  folder any event about the listing lists it again rather than applying sealed names.
- **Upload** (picker and drop): names sealed for their parent, each file encrypted into the
  private folder first (a resumable upload reads its chunks again), then uploaded. New folder,
  rename and moves inside the folder send sealed names; the move picker stays inside the
  protected folder, and the ordinary one does not offer protected folders.
- **Open:** the built-in viewers decrypt in memory and a text edit saves encrypted. "Open with"
  decrypts into the private folder.
- **Download** decrypts each file after it arrives, next to its target.
- **Delete** inside a protected folder, or of a folder that is or holds one, says "Delete for
  good".
- Send, share and details are off for what is in, is or holds a protected folder.
- **Sync:** Protect is not offered for a folder synced on its own (its own mapping to a local
  folder), since protecting would end that sync. A folder synced through a folder above it can be
  protected: before encrypting, the desktop sets it to not sync and waits until the sync reports
  it left out, so the sealed names and deleted plain files never reach this computer. A protected
  folder that a synced folder holds is set to not sync (`Command::Map` with no local folder),
  which stops following it and leaves the local copy as it is.
- **Which key:** protecting and unlocking look for the user's keys among those plugged in, with
  the probe that needs no touch, waiting up to two minutes for one ("Plug in one of the keys you
  already have: “A” or “B”."). The first found is the one used: "Enter the PIN of “A”." To use a
  particular key, the user plugs in only that one. A failed look offers Look again. A key that is
  none of the user's is answered with the names of those that would do.
- **Adding a key with one USB port:** the key that opens the vault and the new key can be plugged
  in together or one after the other. Each step waits up to two minutes for its key to be plugged
  in and the dialog says which ("Plug in “YubiKey”, the key you already have.", then "Unplug
  “YubiKey” and plug in the new key."); Cancel ends the wait.
- **Conversion** (`protected/convert.rs`): the same resumable copy-then-delete as the web's,
  streaming through the private folder, so it has no size limit. It first counts the files left
  to copy and their bytes, then reports each file's step (downloading, encrypting or decrypting,
  uploading) and the bytes moved. A card over the page (`browser/conversion.rs`) shows the
  folder, "File 3 of 12 · 42%", a bar, the step with the bytes and the time left, or why it
  stopped with Finish. The folder does not open while it runs: the folder shown leaves it for
  its parent, and a link or a search into it lands on the parent too. A folder that finishes
  protecting is locked at once, since its keys were only kept open to encrypt. After protecting, it
  offers to delete for good the folder's files that were in the recycle bin from before.

**Security-key folders on the desktop (done 2026-11-28, Linux and macOS)**, in
`protected/security_key.rs` (the key) and `browser/protect/keys.rs` (the dialogs), with the
shared requests in `client_core::protected` (`with_vault`, `enable_folder_key`,
`new_vault_recovery_key`, `protect_with_vault`, `open_with_vault`):

- **Settings → Account** (opened from the account menu, as on the web: the user, Settings, Lock
  all, Log out, the version) has Profile, Change password and "Two-factor sign-in and passkeys":
  the authenticator app (set up with its QR code, removed), the recovery codes, the sign-in keys
  (listed, renamed, removed; added in the web app, since the desktop has no WebAuthn) and the
  **Encryption keys**: listed with when each was added, Add an encryption key…, Remove (`409
  last-unlocker` shows the server's text), and Create or Replace the recovery key (shown once,
  with Copy and Save to a file). Keys are registered only here. A change the server wants
  confirmed asks for the password or a code in a dialog and runs again.
- **Add a key:** the first key starts the vault (two touches: make the credential, then its
  PRF). Another opens the vault in the same step, so the vault key never outlives one action:
  with a key the user has, plugged in beside the new one (its PIN, one touch, then two on the new
  key), or with the recovery key. The new key is the only plugged-in key that holds none of the
  folder keys; several fresh keys, a key that is already one, a key without a PIN and a passkey
  that syncs (`BE` flag) are refused, each with its own text.
- **Protect with security key…** in a folder's menu, beside Protect with password…, in two steps.
  First what it does (end-to-end encryption) and that the previous versions go; with exactly one
  key and no recovery key a warning that losing it loses the files, and that more keys are
  registered in Settings → Account. With no key it says so, and offers **Go to Account** instead
  of Protect. Protect then looks for a key and asks for its PIN; one touch, then the same
  conversion as a password folder.
- **Unlock…**: looks for a key at once, then its PIN and one touch. "Lost your security key?"
  opens the folder with the vault recovery key instead. The locked placeholder offers Unlock… too.
- **Dialogs** close only by their buttons or Escape, not by a click beside them, so nothing typed
  is lost by a stray click. The system's file dialogs open over the app's window
  (`file_dialog.rs`), so none opens behind it.
- **Unplugging** the key that unlocked a folder locks it: while one is unlocked that way the app
  lists the FIDO devices every 2 s (`KEY_CHECK`), without opening them.
- Every wait for a touch says so in the dialog ("Touch your security key to unlock the
  folder."); the PIN is the app's own field and is kept only for the one action.
- **Windows:** not offered (`security_key::SUPPORTED`); unlocking says that security keys work
  on Linux and macOS for now.

- **Removing a lost key** (done 2026-11-28): Remove asks whether the key is lost or someone
  else may have it; then the vault key is replaced in the same step (`POST /vault/rotate`,
  `client_core::protected::rotate_vault`), so a copy of the vault from before opens nothing,
  even with that key and its PIN. Every key that stays is plugged in at once, each with its PIN
  (`security_key::KeyRing`): the first opens the vault and is touched twice, every other key
  once. Each security-key folder's secret is wrapped anew; no file is re-encrypted. The recovery
  key is replaced too and the new one shown once, except when it opened the vault (no key
  stays), when it is kept.
- **Cancel while waiting for a touch** (done 2026-11-28) tells the key to stop
  (`CTAPHID_CANCEL`, `security_key::cancel`), so it is free at once; an answer that comes
  after the user cancelled is dropped.
- **Plain copies** (done 2026-11-28): "Open with" and Download inside an unlocked protected folder
  say first that the copy is not protected; agreeing holds until the folder locks or is unlocked
  again.
- **A folder protected elsewhere** (done 2026-11-28): the server announces every change of a
  folder's protection as the `protection_changed` event (protecting, finishing, unprotecting,
  removing, a new password, a vault rotation). The app sets a folder protected elsewhere to not
  sync as soon as the event comes, before the other client's conversion seals its names, then
  reads `GET /protected` again; the web client reloads its list too.

Still to do on the desktop:

- **Windows:** the WebAuthn API (`webauthn.dll`) for security keys.
- **macOS:** not tried; the `home-space://` link to a running app needs the Apple Event.
- The on-demand files (`clients/vfs-core`) do not exist yet; they must leave out the folders
  `GET /protected` lists.

**Opened from the browser** (decided 2026-11-27; done 2026-11-28 except macOS): the app registers
the `home-space` URL scheme with the operating system: `MimeType=x-scheme-handler/home-space` and
`Exec=HomeSpace %u` in the `.desktop` file (Flatpak too), `osx_url_schemes` in the bundle on
macOS, and `HKCU\Software\Classes\home-space` through `reg add` at every start on Windows. The web
client links a security-key folder as `home-space://folder/<id>?server=<origin>`
(`client_core::app_link`); the app checks that `server` has the origin it is signed in to (a link
that comes before sign-in waits for it), opens the folder, finds it locked and asks to unlock it.
A second start with a link hands it to the running app (`desktop/src/app_link.rs`): the running
app listens on a loopback port named, with a random token, in a file of its data folder that only
the user can read, and takes a link only with the token. Not done: macOS hands the link to a
running app as an Apple Event (`kAEGetURL`), which the app does not handle yet; the Windows
registration is not tested on Windows.

## Web client

Decided 2026-11-27: the web client unlocks **password folders only**. Security keys are left to
the desktop and mobile apps, so the browser never holds a vault key.

- **Password folders:** Protect with password, Unlock, Lock, Lock all, Change password and
  "Forgot the password?" with the recovery key, Unprotect, and Finish protecting or unprotecting
  a conversion that stopped (done). Inside an unlocked folder: listings with the names opened,
  encrypted uploads, new folders and renames, moves within the folder (each name sealed again for
  its new parent), decrypted downloads, the built-in viewers (text editing saves encrypted), and
  "Delete for good" (done). After protecting, it offers to delete for good the folder's files that
  were in the recycle bin from before, which are still plain (done).
- **Security-key folders** show locked, with **Open in the desktop app**, a
  `home-space://folder/<id>?server=<origin>` link (see [Desktop client](#desktop-client-phase-2)).
- **Decrypt:** `vault-core` compiles to WASM with the Leptos app; nothing needs WebCrypto. The
  page encrypts and decrypts whole files in memory, up to 512 MB; larger ones need an app.
- **Hand out:** viewers get `Blob` URLs or bytes in memory, closed and revoked on lock; a
  download is a `Blob` of the decrypted file. Decrypted files are only ever shown as a type from a
  short list, never as HTML or SVG.

**The caveat:** the web client's code comes from the same server that the encryption is meant to
protect against. A server operator, or an attacker who controls the server, can serve a modified
client that sends the password or the plaintext back after the user unlocks. Desktop and mobile
apps do not have this problem: they are signed releases that do not change when the server
changes. This is the same limit every web-based end-to-end encrypted service has; the Protect
dialog says so.

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
- The mobile file providers never show protected folders.

## Phases

1. **Foundation** (server + `vault-core`): `vault-core` with fixed test vectors (done); schema,
   vault and protected folder API, folder keys apart from sign-in keys, `409 last-unlocker`, the
   protecting states (done); the web client's folder key list (done); the Authenticator for
   Linux (done 2026-11-28, `ctap-hid-fido2`), macOS and Windows.
2. **Desktop, security keys**: enabling keys, recovery key, the Protect dialog, protecting new and
   existing folders, Unlock / Lock / Lock all, auto-lock, browse, open, upload, download, move and
   rename inside, deleting for good, hiding from on-demand files and sync, the `home-space://`
   scheme for folders handed over from the browser, replacing the vault key after losing a key
   (done 2026-11-28 on Linux, see [Desktop client](#desktop-client-phase-2); macOS and Windows
   still to do).
3. **Password folders, end to end** (2026-11-27): Argon2id-wrapped folder secrets and per-folder
   recovery keys in `vault-core`, the server storing them (done); the shared groundwork in
   `api-client` and `client-core` (`client_core::protected`: unlocked keys with the
   inactivity timer, sealed names, the password lock's requests) (done); the web client, as
   [Web client](#web-client) lists (done). Protecting a folder with content copies each file
   encrypted and then deletes the plain one, so it can stop and continue ("Finish protecting")
   without a journal; unprotecting does the same the other way. Then the desktop (done
   2026-11-28, see [Desktop client](#desktop-client-phase-2)). No hardware needed.
4. **Desktop polish**: external-app save-back, unprotect.
5. **Android**, then **iOS**.

## Testing

- `vault-core`: fixed test vectors for the KEK derivation, PRF salt transform, name sealing and
  the folder recipient (done); round trips of names and content, including seeking (done).
- A `vault-core` software `Authenticator` drives the vault without hardware (done), so the e2e
  tests (`e2e/`) can drive the whole flow. The server takes no part in folder key ceremonies, so
  its tests post unlockers as a client would.
- Cross-client check: a folder key made in Chrome locks a folder on the desktop, and a file
  protected by the desktop opens in Chrome with the same key (manual, with a real YubiKey, before
  each phase ships).
- Server tests (done for phase 1): folder keys only whole and for this server's hosts, sign-in
  keys and an administrator's reset of second factors leaving the vault alone, `409 last-unlocker`,
  rotation needing every folder and the current epoch, protection never overlapping, the states
  of a conversion, recent sign-in for every change of what unlocks.
- Password folders (done): the server keeps exactly what the client wrapped and refuses
  parameters out of bounds, a new password keeps the recovery copy unless replaced, nothing
  moves across a protected folder's edge, nothing lands there unencrypted or renumbered, nothing
  inside is indexed, shared or sent, deletes skip the bin. `vault-core`: a wrong password does
  not unwrap, NFC and NFD passwords agree, the recovery key opens and is replaced when used, a
  swapped recipient is caught, known-answer vectors for both KEKs.

## Later

- **Write without a touch:** the folder's X25519 recipient is public, so a client could encrypt
  new files into a security-key folder without the key (for example mobile auto-upload into a
  protected "Scans" folder). Names would be sealed to the recipient instead of with AES-SIV and
  re-sealed on the next unlock.
- **Sharing a protected folder with another user:** wrap the folder secret to that user's vault
  public key.
- **Rollback detection:** a signed, versioned manifest per folder so a client notices when the
  server withholds or rolls back files.
- **Seal the root name:** it stays in clear for now (decided 2026-11-27), so the listing shows
  which folder is protected.
- **Security keys in the browser:** WebAuthn PRF would allow it, behind an opt-in per vault; left
  out (2026-11-27) so the vault key never meets code the server serves.

## Decided questions

Answered 2026-11-27:

1. The protected folder's own name stays in clear.
2. Auto-lock after 15 minutes without activity, one constant for every client
   (`client_core::protected::AUTO_LOCK_AFTER`).
3. No opt-in: the web client does not use security keys at all; it hands such folders to the
   desktop app through `home-space://`.
4. Minimum hardware: any FIDO2 key with `hmac-secret` and a PIN.
5. Password folders work in the web client from the start, with the caveat in the Protect dialog.
6. Names in protected folders have the same limit as anywhere else, 255 characters; the server
   accepts their sealed text, up to 1387 characters, inside protected folders only.
