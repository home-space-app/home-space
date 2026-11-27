//! Known-answer vectors: other clients (the web, Kotlin, Swift) must produce exactly these, and a
//! change here means files written before no longer open.

use crate::authenticator::PrfOutput;
use crate::folder::FolderSecret;
use crate::kdf::{Kek, prf_salt_input};
use crate::names::NodeKind;
use crate::password::{MIN_ITERATIONS, MIN_MEMORY_KIB, PasswordKdf};
use crate::recovery::RecoveryKey;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const PARENT: uuid::Uuid = uuid::Uuid::from_bytes([0x11; 16]);

#[test]
fn prf_salt_transform() {
    // Also SHA-256("WebAuthn PRF\0"), as `shasum` computes it.
    assert_eq!(
        hex(&prf_salt_input(b"")),
        "6a7e64b2aa34c92736143a062fa149aff1bd8bb3f7ee6f346885481f9414a3d3"
    );
    assert_eq!(
        hex(&prf_salt_input(&[0x42; 32])),
        "13c28746a9a94668e9b2bf04e35d7d7d6970db00b36ff7002424f76eb424a16d"
    );
}

#[test]
fn keks() {
    assert_eq!(
        hex(Kek::from_prf(&PrfOutput::from_bytes([0x01; 32]))
            .unwrap()
            .expose()),
        "74c795a97a17da504db4dcbecdb65a46944309b565566b91690ff04a46a7137e"
    );
    assert_eq!(
        hex(Kek::from_recovery(&RecoveryKey::from_bytes([0x02; 32]))
            .unwrap()
            .expose()),
        "1240aeb8c3667c9bffee4cc72d8bf6eda317fc843dddb26877aa3924ccb0fec5"
    );
}

#[test]
fn password_keks() {
    let kdf = PasswordKdf {
        salt: [0x06; 16],
        memory_kib: MIN_MEMORY_KIB,
        iterations: MIN_ITERATIONS,
        parallelism: 1,
    };
    assert_eq!(
        hex(Kek::from_password("correct horse", &kdf).unwrap().expose()),
        "2e6244a1cf857782328f000a0fc2c7905e86b5be6b91ff142aec956582d59671"
    );
    assert_eq!(
        hex(
            Kek::from_folder_recovery(&RecoveryKey::from_bytes([0x02; 32]))
                .unwrap()
                .expose()
        ),
        "2b3ed7ccd8af01d027ecefbced19af60ad3a7a28bbc1e33a70d901d42949d462"
    );
}

#[test]
fn folder_keys() {
    let keys = FolderSecret::from_bytes([0x04; 32]).keys().unwrap();
    assert_eq!(
        keys.recipient().to_string(),
        "age1fauk43xvke3myu0vg0syaflnwhlslwxd2504jfdjwejkjc50pcysgcf5gf"
    );
    assert_eq!(
        keys.seal_name(&PARENT, NodeKind::File, "Passport.pdf")
            .unwrap(),
        "tj7VHBiK2oF-eDnpgp214PuYUBMW9PAxOeKGlIRkQRdPRbxdbU-EXVDanL-aojUT"
    );
}

#[test]
fn recovery_key_text() {
    assert_eq!(
        &*RecoveryKey::from_bytes([0x05; 32]).to_display(),
        "AUCQK-BIFAU-CQKBI-FAUCQ-KBIFA-UCQKB-IFAUC-QKBIF-AUCQK-BIFAU-CWAAI"
    );
}
