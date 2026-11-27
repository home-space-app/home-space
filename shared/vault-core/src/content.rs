//! File content: age v1 with the folder's X25519 recipient.
//!
//! Encrypting needs only the public recipient, so uploads need no key. Decrypting uses the
//! identity once, to unwrap the file key in the header; the returned reader holds only that
//! file's payload key, so the folder keys can be dropped right after.

use std::io::{self, Read, Write};

use age::stream::{StreamReader, StreamWriter};
pub use age::x25519::{Identity, Recipient};

use crate::{ContentError, Result};

/// Every age v1 file starts with this line; the server checks it on uploads into protected
/// folders.
pub const AGE_HEADER_PREFIX: &[u8] = b"age-encryption.org/v1\n";
/// Room for the age header and the per-chunk tags of a small file, so encrypting a buffer does
/// not reallocate.
const AGE_OVERHEAD_HINT: usize = 256;

/// Whether `prefix` (the first bytes of a blob) starts like an age v1 file.
pub fn looks_like_age(prefix: &[u8]) -> bool {
    prefix.starts_with(AGE_HEADER_PREFIX)
}

/// A folder's recipient from its text form (`age1…`).
pub fn parse_recipient(text: &str) -> Result<Recipient> {
    text.trim()
        .parse()
        .map_err(|reason| ContentError::Recipient(reason).into())
}

/// A writer that encrypts into `output`. Call [`StreamWriter::finish`] at the end, or the last
/// chunk is missing and the file does not decrypt.
pub fn encryptor<W: Write>(recipient: &Recipient, output: W) -> Result<StreamWriter<W>> {
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
            .map_err(ContentError::from)?;
    Ok(encryptor.wrap_output(output)?)
}

/// Encrypts all of `input` into `output`, streaming; returns the plaintext size.
pub fn encrypt<R: Read, W: Write>(recipient: &Recipient, mut input: R, output: W) -> Result<u64> {
    let mut writer = encryptor(recipient, output)?;
    let size = io::copy(&mut input, &mut writer)?;
    writer.finish()?;
    Ok(size)
}

/// Encrypts a buffer.
pub fn encrypt_bytes(recipient: &Recipient, plain: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(plain.len() + AGE_OVERHEAD_HINT);
    encrypt(recipient, plain, &mut out)?;
    Ok(out)
}

/// Opens an age file. The reader decrypts as it is read; with an `input` that implements `Seek`
/// it seeks too (range reads, for video), decrypting only the chunks it needs.
pub fn decrypt<R: Read>(identity: &Identity, input: R) -> Result<StreamReader<R>> {
    let decryptor = age::Decryptor::new(input).map_err(ContentError::from)?;
    Ok(decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(ContentError::from)?)
}

/// Decrypts a buffer.
pub fn decrypt_bytes(identity: &Identity, encrypted: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(encrypted.len());
    decrypt(identity, encrypted)?.read_to_end(&mut out)?;
    Ok(out)
}

/// Whether `input` is an age file this identity opens: its header unwraps with it. Reads only
/// the header. A plain file, or one encrypted to someone else, is not.
pub fn opens_with<R: Read>(identity: &Identity, input: R) -> bool {
    age::Decryptor::new(input)
        .and_then(|decryptor| decryptor.decrypt(std::iter::once(identity as &dyn age::Identity)))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Seek, SeekFrom};

    use super::*;
    use crate::FolderSecret;

    #[test]
    fn only_its_own_files_open_with_an_identity() {
        let ours = FolderSecret::generate().keys().unwrap();
        let theirs = FolderSecret::generate().keys().unwrap();
        let sealed = encrypt_bytes(&ours.recipient(), b"scan").unwrap();
        assert!(opens_with(ours.identity(), sealed.as_slice()));
        assert!(!opens_with(theirs.identity(), sealed.as_slice()));
        assert!(!opens_with(ours.identity(), &b"plain text"[..]));
        assert!(!opens_with(ours.identity(), &b""[..]));
    }

    #[test]
    fn round_trip_with_only_the_recipient_to_encrypt() {
        let keys = FolderSecret::generate().keys().unwrap();
        let recipient: Recipient = keys.recipient().to_string().parse().unwrap();
        let plain: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let encrypted = encrypt_bytes(&recipient, &plain).unwrap();
        assert!(looks_like_age(&encrypted));
        assert_ne!(
            encrypted,
            encrypt_bytes(&recipient, &plain).unwrap(),
            "a new file key each time"
        );
        assert_eq!(decrypt_bytes(keys.identity(), &encrypted).unwrap(), plain);
    }

    #[test]
    fn range_reads_after_dropping_the_keys() {
        let keys = FolderSecret::generate().keys().unwrap();
        let plain: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        let encrypted = encrypt_bytes(&keys.recipient(), &plain).unwrap();
        let mut reader = decrypt(keys.identity(), Cursor::new(encrypted)).unwrap();
        drop(keys);
        reader.seek(SeekFrom::Start(150_000)).unwrap();
        let mut buf = vec![0; 1000];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(buf, plain[150_000..151_000]);
    }

    #[test]
    fn wrong_identity_and_tampering_fail() {
        let keys = FolderSecret::generate().keys().unwrap();
        let mut encrypted = encrypt_bytes(&keys.recipient(), b"secret").unwrap();
        assert!(
            decrypt_bytes(
                FolderSecret::generate().keys().unwrap().identity(),
                &encrypted
            )
            .is_err()
        );
        let last = encrypted.len() - 1;
        encrypted[last] ^= 1;
        assert!(decrypt_bytes(keys.identity(), &encrypted).is_err());
        assert!(!looks_like_age(b"plain text"));
    }
}
