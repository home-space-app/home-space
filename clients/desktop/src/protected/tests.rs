use protocol::{FolderLock, NodeKind, ProtectionState};
use time::OffsetDateTime;
use uuid::Uuid;
use vault_core::FolderSecret;

use super::*;

const ROOT: NodeId = NodeId(Uuid::from_bytes([1; 16]));
const INNER: NodeId = NodeId(Uuid::from_bytes([2; 16]));

fn keys() -> FolderKeys {
    FolderSecret::generate().keys().expect("keys")
}

fn listed(keys: &FolderKeys) -> ProtectedFolder {
    ProtectedFolder {
        node_id: ROOT,
        lock: FolderLock::Password,
        wrapped_secret: Vec::new().into(),
        epoch: None,
        kdf: None,
        recovery_wrapped_secret: None,
        recipient: keys.recipient().to_string(),
        format: 1,
        state: ProtectionState::Protected,
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn file(parent: NodeId, name: &str) -> Node {
    Node {
        id: NodeId(Uuid::from_bytes([9; 16])),
        parent_id: Some(parent),
        kind: NodeKind::File,
        name: name.to_owned(),
        size: 0,
        sha256: None,
        mime: "application/octet-stream".to_owned(),
        version: 1,
        created_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
        edited_on: None,
    }
}

#[test]
fn an_unlocked_folder_hands_out_its_keys_until_it_locks() {
    let keys = keys();
    let folder = listed(&keys);
    let mut protected = Protected::default();
    protected.loaded(Listed {
        folders: vec![folder.clone()],
        ancestors: HashMap::new(),
    });
    assert_eq!(protected.lock_of(ROOT), Some(false));
    assert!(protected.inside(ROOT).is_none());

    protected.unlock(&folder, Arc::new(keys)).expect("unlocked");
    assert_eq!(protected.lock_of(ROOT), Some(true));
    assert!(protected.inside(ROOT).is_some());

    assert_eq!(protected.lock_all(), [ROOT]);
    assert!(protected.inside(ROOT).is_none());
}

#[test]
fn a_folder_no_longer_listed_locks() {
    let keys = keys();
    let folder = listed(&keys);
    let mut protected = Protected::default();
    protected.remember(folder.clone());
    protected.unlock(&folder, Arc::new(keys)).expect("unlocked");
    assert_eq!(protected.loaded(Listed::default()), [ROOT]);
    assert!(!protected.is_unlocked(ROOT));
}

#[test]
fn a_folder_holding_a_protected_one_is_told_apart() {
    let mut protected = Protected::default();
    protected.loaded(Listed {
        folders: vec![listed(&keys())],
        ancestors: HashMap::from([(ROOT, vec![INNER])]),
    });
    assert!(protected.is_or_holds(ROOT));
    assert!(protected.is_or_holds(INNER));
    assert!(!protected.is_or_holds(NodeId(Uuid::from_bytes([3; 16]))));
}

#[test]
fn inside_seals_names_it_then_opens_with_their_type() {
    let inside = Inside::new(ROOT, ProtectionState::Protected, keys());
    let sealed = inside
        .seal(Some(INNER), NodeKind::File, "scan.pdf")
        .expect("sealed");
    let opened = inside.open_node(file(INNER, &sealed));
    assert_eq!(opened.name, "scan.pdf");
    assert_eq!(opened.mime, "application/pdf");
    assert!(inside.seal(None, NodeKind::File, "scan.pdf").is_err());
}

#[test]
fn content_encrypted_in_memory_decrypts_with_the_same_folder_only() {
    let inside = Inside::new(ROOT, ProtectionState::Protected, keys());
    let other = Inside::new(ROOT, ProtectionState::Protected, keys());
    let sealed = files::encrypt_bytes(&inside, b"taxes").expect("encrypted");
    assert_eq!(
        files::decrypt_bytes(&inside, &sealed).expect("decrypted"),
        b"taxes"
    );
    assert!(files::decrypt_bytes(&other, &sealed).is_err());
}

#[tokio::test]
async fn a_file_encrypted_on_disk_decrypts_to_the_same_bytes() {
    let inside = Inside::new(ROOT, ProtectionState::Protected, keys());
    let dir = std::env::temp_dir().join(format!("home-space-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (plain, sealed, back) = (dir.join("plain"), dir.join("sealed"), dir.join("back"));
    std::fs::write(&plain, vec![7u8; 200_000]).expect("written");

    let size = files::encrypt_file(inside.recipient(), plain.clone(), sealed.clone())
        .await
        .expect("encrypted");
    assert!(size > 200_000);
    files::decrypt_file(inside, sealed, back.clone())
        .await
        .expect("decrypted");
    assert_eq!(std::fs::read(&back).expect("read"), vec![7u8; 200_000]);
    std::fs::remove_dir_all(dir).expect("cleaned");
}
