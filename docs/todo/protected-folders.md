# Protected folders: end to end with a security key, or server-encrypted with a password

## Problem

Everything a user stores is readable by whoever runs, backs up or breaks into the server: file
content on disk (or in object storage, see `object-storage.md`), names in `nodes`, and extracted
text in `search_entries`. Some folders (identity documents, tax papers, medical records, keys)
should be readable only on the user's own devices, or at least only after a second secret.

The user picks a folder and clicks **Protect**, choosing one of two locks:

- **Security key** (recommended): one of the user's folder keys, security keys (a YubiKey or any
  FIDO2 key) enabled for the vault (see [Folder keys](#folder-keys-credentials-of-their-own)).
  **End to end**: the client encrypts
  names and content, and someone with the server and all its data still cannot decrypt the folder;
  the secret is on the key, behind its PIN.
- **Password** (less secure): a password chosen for the folder. **Not end to end**: the server
  encrypts the folder with a key it keeps, and opens it to whoever gives the password. It protects
  against people who get at the stored files, or at the user's signed-in apps, but not against the
  server itself.

Both add **Unlock** and **Lock** to the folder, and both lock themselves after inactivity and when
the app closes. Desktop first, mobile later; the web client can do security-key folders opt-in
and with a trust caveat ([Web client](#web-client)).

## The two locks

Decided 2026-11-26.

| | Security key | Password |
|---|---|---|
| Who encrypts | The client (`shared/vault-core`) | The server |
| What the server can read | Nothing: names and content are ciphertext it stores like any other bytes | Everything, while it has the key (always) |
| Key | Folder secret wrapped by the user's vault, which each folder key (and an optional recovery key) unwraps through WebAuthn PRF / `hmac-secret` | Random folder key, kept sealed with the server's secrets key; the password is checked against an Argon2id hash |
| Protecting an existing folder | The client downloads the files, deletes them from the server (for good, see below), and uploads them encrypted. **Their version history is lost**, and the user is warned first. | The server encrypts the content and every old version in place; history is kept. |
| Names | Sealed by the client (AES-SIV), stored as base64url text in the normal `name` | Encrypted by the server, shown to sessions that unlocked the folder |
| Unlock | PIN and touch; decrypts the names only. Every open, upload or rename needs another touch: **no key stays in the client's memory**. | Password, sent to the server; the session sees the folder until Lock or a timeout |
| Old clients and on-demand files | See meaningless names (base64url); new clients hide protected folders there | See nothing inside a locked folder (`423 locked`) |
| Deleting | **For good**, versions included: nothing inside goes to the recycle bin | The same |

## Threat model

Protects against:

- Someone who reads the server's disk, database, object storage bucket or backups:
  - **Security-key folders:** fully, and also a server operator or an attacker with the server
    who reads data passively. What the server holds (credential ids, salts, wrapped keys,
    ciphertext) is useless without the physical key and its PIN.
  - **Password folders:** someone with the stored files (blobs, object storage, a disk) but not
    the database **and** the server's secrets key (`HOMESPACE_SECRETS_KEY`, kept outside the
    database like the one for authenticator app secrets). With all three they can decrypt without
    the password.
- Someone who steals a laptop with an app signed in, or an access token: a security-key folder
  cannot be opened without the key and its PIN, a password folder not without its password.
  Everything that changes what unlocks a folder needs a recent sign-in.
- Someone who steals the security key: without its PIN the key does not answer (FIDO2 user
  verification is required, and the key locks itself after 8 wrong PINs).
- Malware that reads the app's memory **while a security-key folder is unlocked but no file is
  being opened**: it finds names, not keys (see [Unlock and lock](#unlock-and-lock)).

Does **not** protect against:

- **Password folders: the server.** Its operator, or an attacker who controls it, can read the
  folder whenever they like. The Protect dialog says so in one sentence.
- An attacker controlling a client while the user works in it (malware on the desktop, a
  malicious browser extension). They can see what the user opens and can wait for the next touch.
- A server that serves a **modified web client**. This only affects the web client; see
  [Web client](#web-client).
- Metadata of security-key folders. The server still sees: which folder is protected, how many
  files and folders it has and how they nest, file sizes (rounded only by the 64 KiB chunks plus a
  small header), how many versions each file has, when files are created and changed, which
  device changed them, and that names within one folder differ. It does not see names, MIME types
  or content.
- Deletion. The server can still delete or roll back ciphertext. Tampering is detected (AEAD), but
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

- A credential made with `navigator.credentials.create` (web) or CTAP2 `makeCredential`
  (native) with a challenge the client makes itself: the server takes no part in its ceremonies
  and never checks an attestation or an assertion. Not discoverable (`residentKey:
  discouraged`), always with user verification, and with the `prf` extension (`hmac-secret`),
  asked for the new salt at once. The user handle is random, never the account's, so a key that
  keeps the credential anyway cannot replace a sign-in passkey with it.
- On a hardware key: the web client asks for a security key (`hints: ["security-key"]`,
  `cross-platform`) and refuses a new credential whose authenticator data says it is backed up
  (a synced passkey), since the native apps cannot reach those.
- The key needs a PIN (or fingerprint): `hmac-secret` gives a different secret with and without
  user verification, so unlocking always asks for it.
- The server keeps it as an unlocker (`vault_unlockers`): its name (at most 64 characters, like
  a sign-in key's), credential id, RP id, PRF salt and wrapped vault key. The RP id must be one
  of the tenant's names, the host of its `public_url` or one of its domains, or, for a tenant
  with neither, the host the request was sent to (`browser::WebAuthnSites`, as for passkeys).

Sign-in ceremonies never ask for PRF, so a PRF value never reaches the server, and
`GET /me/webauthn` says nothing about it.

### Enabling a key

- **Web:** the security page lists **Sign-in keys** and **Folder keys** apart (done), and removes
  a folder key (done). "Add a folder key" makes the credential
  (`clients/web/src/webauthn/folder_key.rs`, done), takes its PRF secret from the answer to
  `create` when the browser gives it, else from one more assertion, wraps the vault key for it and
  sends `POST /vault/unlockers`. The vault key is new for the first key; for another, an existing
  folder key opens the vault first (one assertion listing them all, each with its salt through
  `evalByCredential`), or the recovery key. **To do:** the wrapping, which waits for
  `vault-core` to take a PRF secret the browser gave (its `Authenticator` trait is synchronous,
  `navigator.credentials` is not, and `PrfOutput` has no public constructor).
- **Desktop:** "Security keys for protected folders" lists the folder keys (`GET /vault`).
  **Enable** makes a new credential with CTAP2 `makeCredential` for the server's host, with
  `hmac-secret`, then wraps the vault key for it as above. Until the native authenticators exist,
  it opens the web security page in the browser.

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
  uploads: whenever a client has the folder secret (every touch), it derives the recipient and
  refuses a listed one that differs (`FolderKeys::check_recipient`).
- **File key**: age generates a random one for every file it encrypts, so every version of every
  file has its own. It is wrapped to the folder's recipient in the age header, inside the blob.

### Lost keys and recovery

Nothing is forced (decided 2026-11-26):

- The Protect dialog offers **Security key (recommended)** and **Password (less secure, not end
  to end)**.
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
  column, no check that it is encrypted. Base64url holds neither `/` nor `\`, and at most five
  padded blocks plus the tag are 235 characters, within the 255 the server allows. So a name in a
  security-key folder can be **at most 159 bytes of UTF-8** (159 ASCII characters, about 53 in
  most Asian scripts, 39 emoji), which the client checks before sealing.
- **Case:** names compare by bytes, as today. Unicode is normalized to NFC before sealing, so
  macOS (NFD) and the others agree.

**The protected folder's own name** stays in clear, so the user sees "Taxes 🔒" in the listing
without the key. Everything inside it is sealed. **Open question:** offer to seal the root name
too.

### The server treats them like any other folder

Decided 2026-11-26: the server does no cryptography for security-key folders and does not tell
their nodes apart. It is the client's job to encrypt; the server stores and serves bytes. Someone
who lists such a folder without the key (an old client, the web client without opt-in, the API)
sees names with no meaning and can count the files, nothing more.

What the server does:

- Keeps the vault, the unlockers and the `protected_folders` rows (see [Server](#server)).
- Ends the sharing of the folder and everything in it when it is protected.

What follows for the other features, without special cases on the server:

| Feature | In a security-key folder |
|---|---|
| Search | The server indexes nothing inside a protected folder of either lock. Inside an unlocked folder the client searches the decrypted names in memory. |
| Previews, gallery, viewers | Only in the client, after decryption; a gallery of many images is one touch for the visible page. |
| Shares, transfers, archive downloads | Refused for a protected folder and what it holds (`409 protected`), for both locks, since they would hand out ciphertext. A shared folder or a folder sent to a device that holds a protected folder leaves it out, in listings, archives and copies. Sharing encrypted folders with other users is under [Later](#later). |
| Moves and copies | Work as today. Within the folder the client sends the name sealed for the new parent. Into or out of it, the client copies with encryption (or decryption) and deletes the source; a plain move would leave plaintext inside or ciphertext outside. |
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
4. With one touch it seals every name, then encrypts each file to the folder's recipient and
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

## Password folders (server-side encryption)

Decided 2026-11-26: a password folder is the server's. Clients send the password and receive
plaintext over TLS, like any other folder once it is unlocked.

Done (stage A, content):

- **Protect:** `POST /protected` `{ node_id, lock: "password", password }` (recent sign-in; the
  password follows the account password rules). The server generates a random folder secret,
  stores it sealed with its secrets key (`secret_box`, as for authenticator app secrets; without
  `HOMESPACE_SECRETS_KEY` password folders are refused), the folder's age recipient derived from
  it, and an Argon2id hash of the password to check it against. The key is not derived from the
  password, so changing the password will re-encrypt nothing. The session that protects the
  folder has it unlocked, as after an unlock.
- **Format:** the same as security-key folders: content is age files to the recipient
  (`shared/vault-core`, which the server uses with its `server` feature). The listed size and
  SHA-256 stay those of the plain content, since that is what clients receive.
- **Writes:** whatever is stored inside a password folder (uploads, new versions, copies) is
  encrypted to the recipient before it lands in place.
- **Existing content:** with content, the folder is `protecting` and the server encrypts the
  current content and **every old version in place**, so history is kept. It runs right after
  protecting and then every minute until done (resuming after a restart), and marks the folder
  `protected`. A blob the folder's identity already opens is skipped, so a run that stopped
  half-way just runs again. The conversion and stores into the folder take turns on one lock, so
  neither overwrites what the other just wrote. Clients cannot `finish` or `unprotect` a password
  folder.
- **Unlock:** `POST /protected/{id}/unlock` `{ password }`, throttled per folder and source like
  share passwords (`422 wrong-password` when wrong). It grants the caller's session the folder
  (`protected_unlocks`) for 15 minutes, extended while it is used; `DELETE
  /protected/{id}/unlock` locks it again, and expired unlocks are swept every minute. Other
  sessions of the same user stay locked out.
- **Reads:** downloads of a file or a version inside answer `423 locked` unless the session (or
  the ticket's session) unlocked the folder; then they are decrypted as they stream, range
  requests included, decrypting only the chunks they need. Blobs not converted yet are sent as
  they are.
- **Moves** into or out of a password folder are refused (`409 protected`): what is inside is
  stored encrypted and what is outside is not. The client copies instead.
- Protecting removes the folder's search entries; new content is indexed from ciphertext, which
  yields no text.

Done (stage B, names and access):

- **Names at rest:** every name inside a password folder (not the folder's own, which its parent
  lists) is stored sealed: the same AES-SIV as security-key folders (`vault-core`), with the
  folder's name key, as base64url text. The associated data is the **protected folder's id**, not
  the direct parent's as for security-key folders, so moving within the folder keeps a sealed
  name valid and folder merges still meet same-named folders. Sealing is deterministic, so the
  unique index still refuses two equal names. The server seals at the edge of its tree logic
  (`files::Names`): numbering a taken name (`plan (1).txt`) and guessing a type from an extension
  work on the plain name.
- **Name length:** the same 159 bytes as in security-key folders, `422 invalid-name` beyond.
  Protecting a folder that holds a longer name is refused (`/node_id`: rename it first); one that
  appears while the folder is being converted is shortened, keeping its extension, and a name
  whose sealed form is taken (equal after Unicode normalization) is numbered.
- **Existing names** are sealed by the same conversion as the content, before it, and skipped
  when they already open.
- **Access:** reading or writing inside a password folder needs the session to have unlocked it,
  else `423 locked`: listing, a node, its ancestors, properties and versions, downloads,
  creating folders, uploads (single and resumable, at every chunk), replacing content, renaming,
  moving, deleting (also the folder itself, with all it holds), and saving a transfer or a copy of
  a shared item into it. The folder's own entry and the listing of its parent stay readable.
  Every such use extends the unlock.
- **Feeds:** the changes journal and the websocket's events leave out nodes inside a folder the
  session has not unlocked, and carry plain names for one it has. Checking does not extend the
  unlock, so a client that only listens lets it expire. A client lists the folder again after
  unlocking it, since the journal went on without it.

Done (stage C, the rest):

- **Left out:** nothing inside a protected folder, of either lock, is indexed for search (names
  or content) or listed among duplicates, which would tell that a plain file has a copy inside.
  Sharing or sending a protected folder or anything in it is refused (`409 protected`); a shared
  folder that holds one does not show it through the share (listing, archive, copy), and sending
  such a folder leaves it out. The server has no previews.
- **Recycle bin:** deleting inside deletes for good, as in every protected folder (see
  [Deleting](#deleting-inside-a-protected-folder)). Deleting a folder that holds a password
  folder also needs every such folder unlocked (`423`).
- **Recovery key:** protecting with a password also makes a recovery key for the folder (the
  vault's printed format: 55 characters in 11 groups), returned once as `recovery_key` in the
  answer to `POST /protected`. The server keeps only its Argon2id hash.
- **Change the password** (decided 2026-11-26): `POST /protected/{id}/password` `{ password,
  current_password }`, or `{ password, recovery_key }` when the password is forgotten, plus a
  recent sign-in. **Signing in to the account is never enough**: someone who has the account's
  password must not get into its folders by resetting theirs. A recovery key that was used is
  replaced, and the answer carries the new one (`recovery_key`), shown once. Wrong passwords and
  keys are throttled like unlocks (`422 wrong-password`). The folder's key stays, so nothing is
  re-encrypted; the caller's session has the folder unlocked, every other one is locked out
  until it unlocks with the new password. Without the password and the recovery key, the files
  are lost.
- **Unprotect:** `POST /protected/{id}/unprotect` for a session that unlocked the folder and
  signed in recently. The folder is `unprotecting`: the server decrypts every blob and opens
  every name in place (same job and lock as protecting), and once a pass finds nothing left it
  drops the protection, the key and the unlocks with it. Meanwhile the folder still needs the
  unlock, new names are refused (`409 protected`, "try again once it is done") and new content
  is stored plain. `DELETE /protected/{id}` stays refused for password folders.
- **Unfinished uploads** into a password folder are listed under their plain names for a session
  that unlocked it, and left out for one that did not.
- **Stores and conversions** take one read-write lock: stores into a password folder hold it
  shared from reading the folder's state until the blob is committed, the conversion exclusively
  per blob. So a store never lands in the form the folder had before protecting or unprotecting
  began.

To do:

- Clients show the recovery key after protecting, ask the user to keep it somewhere safe, and
  offer "Forgot the password?" with it.

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

For security-key folders the client keeps **no decryption key** between actions (decided
2026-11-26). Each action that needs one asks the security key, derives what it needs, does that
one action and wipes every key (`zeroize`) before it returns.

| Action | Security-key folder | Password folder |
|---|---|---|
| **Unlock** (context menu, toolbar, or the "Locked" placeholder when the folder is opened) | PIN and touch. Decrypts the **names** of the folder's subtree, then wipes the keys. | Password, checked by the server. The session can use the folder. |
| Browse, sort, search names | No key: the names are in memory. | The server lists decrypted names. |
| Open, download, export | One touch for the whole selection: unwraps the age file keys of the selected files, then wipes the folder identity. | No prompt. |
| Upload, new folder, rename, move | One touch per action, to seal the names. Content is encrypted to the public recipient without a key. | No prompt. |
| **Lock**, **Lock all** | Wipes the names and the PIN token. | Ends the session's access on the server and wipes the names. |

- **PIN once, touch each time** (decided 2026-11-26). Unlock asks for the PIN; the client keeps
  the key's CTAP2 `pinUvAuthToken` until Lock, so later actions need only a touch. The token is
  not a decryption key: it is useless without the physical key plugged in and touched, and user
  verification stays on, so `hmac-secret` gives the same secret. A CTAP 2.1 key limits how long a
  token may be used (about 10 minutes); after that, or after the key is unplugged, the next action
  asks for the PIN again. On Windows, `webauthn.dll` shows its own dialog and may ask for the PIN
  every time; the app cannot change that.
- **Viewer:** while a file of a security-key folder is open in a viewer, its age file key (not
  the folder secret) stays in memory so a video can seek and a PDF can load pages; it is wiped
  when the viewer closes. The decrypted bytes on screen are in memory too, which cannot be
  avoided.
- **Lock** also revokes viewer buffers and deletes the folder's temporary decrypted files.
  Encrypting uploads in progress finish first: they need only the public recipient.
- **Auto-lock**, per folder:
  - after 15 minutes without activity in that folder (a setting);
  - on sleep and screen lock;
  - when the security key that unlocked it is unplugged (security-key folders);
  - on sign-out;
  - when the app closes.
  On a crash the names and the PIN token die with the process; temporary files are swept at the
  next start. Password folders also lock on the server when the session's access times out.

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
- `protected_folders (node_id, owner_id, lock, wrapped_secret, epoch, recipient, password_hash,
  sealed_key, format, state, created_at)`: a security-key folder has `wrapped_secret`, `epoch`
  and `recipient` (checked to look like `age1` and 58 bech32 characters); a password folder has
  `password_hash` and `sealed_key`. `state` is `protecting`, `protected` or `unprotecting`.
  Wrapped keys and secrets are 73 bytes.

Nothing on `nodes` changes: names and content of security-key folders are stored as they come.

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
  recipient, epoch }`. Protecting refuses the recycle bin, Lost & Found and anything overlapping
  another protected folder (`409 protected`), and ends the sharing of the folder and everything in
  it. An empty folder is `protected` at once, one with content `protecting`.
- `POST /protected/{id}/finish` (`protecting` → `protected`), `POST /protected/{id}/unprotect`
  (`protected` → `unprotecting`) and `DELETE /protected/{id}` (only while `protecting` or
  `unprotecting`).
- Everything that changes what unlocks a folder needs a recent sign-in
  (`require_recent_sign_in`): adding or removing unlockers, rotating, protecting, unprotecting
  and removing a protection. With a stolen access token alone, nobody can add their own unlocker,
  lock the user's files with their own key, or throw away a folder's secret.
- Clients do not put `protected_folders` in `REQUIRES`: without it they hide Protect.

Done for password folders: `lock: "password"` in `POST /protected`, `POST` and `DELETE
/protected/{id}/unlock`, `POST /protected/{id}/password`, `POST /protected/{id}/unprotect`
(decrypted by the server), `423 locked` on every read and write inside a locked folder, the
changes journal and events filtered per session, and `409 protected` on moves across the
folder's edge and on sharing or sending from a protected folder (see
[Password folders](#password-folders-server-side-encryption)).

## Desktop client (phase 2)

`shared/vault-core` (done: no I/O, no UI) is used by desktop, web and mobile-core, and by the
server for password folders:

- wrapping and unwrapping the vault key and folder secrets (`chacha20poly1305`, `hkdf`, `sha2`),
  recovery key encoding;
- `seal_name` / `open_name` (`aes-siv`, `unicode-normalization`), as base64url text;
- streaming encrypt and decrypt, and seekable decrypt, on top of `age`;
- an `Authenticator` trait (`prf(credential, salt)`), implemented by each platform, and a
  software one for tests;
- every secret in a `zeroize` type, with the API shaped so a key lives only inside one call
  (`with_folder_keys(|keys| …)`), not in a struct that outlives the action.

Desktop `Authenticator`:

- **Linux and macOS:** CTAP2 over USB HID with `hmac-secret`. Candidates: `ctap-hid-fido2` and
  `ctap-fido2` (both MIT; the latter documents `hmac-secret` and was tested on a YubiKey 5C), or
  `libfido2` through FFI. **Spike:** pick one; verify the PRF value matches Chrome's for the same
  credential and salt, that a folder key made in the browser works, and that the PIN token
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
  described under [Lost keys and recovery](#lost-keys-and-recovery), with the history warning for
  a folder that has content.
- A protected folder shows a lock icon, open or closed. Opening it while locked shows "Locked"
  with an Unlock button. "Unlock" and "Lock" are in the context menu and the toolbar, and
  "Lock all" in the app menu. PIN entry is the app's own dialog on Linux and macOS, the system's
  on Windows. Every action that needs a touch says "Touch your security key" with the action's
  name, so a touch is never asked without a reason the user can see.
- Auto-lock needs two things the desktop does not have today: an inactivity timer (a new iced
  subscription in `browser/mod.rs`) and window close handling (`window::close_requests`; today
  `browser/input.rs` has none).
- **Open:** viewers (images, text, PDF, video) decrypt into memory. "Open with…" an external app
  needs a file on disk: it is decrypted into the private temporary directory, deleted on lock and
  on app exit, with a warning the first time. Saving back from an external app comes in phase 3
  (watch the file, re-encrypt, upload as a new version).
- **Download** is an explicit export to a place the user picks; the app warns that the copy is
  not protected.
- **Upload** into an unlocked protected folder (drag and drop, file picker) asks one touch to seal
  the names, then encrypts while uploading.
- The on-demand files (`clients/vfs-core`) and folder sync (`sync-core`) leave out the folders
  `GET /protected` lists.

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

Proposal: support the web client for security-key folders, but as an **opt-in per vault** ("Allow
unlocking in the browser", off by default, explained in one sentence). Password folders work in
the web client like anywhere else, since the server decrypts them anyway. Phase 4, after desktop.

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
   protecting states (done); the web client's folder key list and ceremonies (done) and enabling
   one (waits for `vault-core`, see [Enabling a key](#enabling-a-key)); Authenticator spikes for
   Linux, macOS and Windows.
2. **Desktop, security keys**: enabling keys, recovery key, the Protect dialog, protecting new and
   existing folders, Unlock / Lock / Lock all, auto-lock, browse, open, upload, download, move and
   rename inside, deleting for good, hiding from on-demand files and sync.
3. **Password folders**: the server's encryption of content, unlock and lock (stage A, done);
   names and locked listings (stage B, done); search, shares, transfers, password change,
   unprotect (stage C, done); then the clients.
4. **Desktop polish**: external-app save-back, unprotect.
5. **Web** (opt-in), with Chrome, Edge and Firefox in the e2e tests (a virtual authenticator with
   PRF in Chrome DevTools Protocol / Playwright).
6. **Android**, then **iOS**.

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
- Password folders: no plaintext blob, name or search entry left after the encryption job, every
  version still downloadable, `423` for every read and write while locked, access ending on Lock
  and after the timeout, the changes journal and events leaving out what a session has not
  unlocked, nothing indexed, shared or sent from inside, deletes skipping the bin, a password
  change locking other sessions out, and unprotecting leaving plain blobs and names (done).

## Later

- **Write without a touch:** the folder's X25519 recipient is public, so a client could encrypt
  new files into a security-key folder without the key (for example mobile auto-upload into a
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
3. Web client: opt-in per vault for security-key folders (proposed), or not at all?
4. Minimum hardware: any FIDO2 key with `hmac-secret` and a PIN (proposed), or YubiKey only?
5. Names in protected folders, of both locks, are limited to 159 bytes. Enough, or should the
   server allow longer names (which some file systems on the desktop could not hold anyway)?
