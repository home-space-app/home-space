//! Starting an upload: where its bytes will go, and the row and file that receive them.

use uuid::Uuid;

use db::{NewUpload, NodeId, NodeKind, UserId};
use protocol::TransferState;

use super::{UploadRequest, UploadService, UploadStatus, UploadTarget, slots::Hashing};
use crate::{FilesError, StoreOptions, tree::guard};

/// Where an upload's bytes go once they have all arrived.
#[derive(Default)]
struct Placement {
    parent_id: Option<NodeId>,
    node_id: Option<NodeId>,
    name: String,
    options: StoreOptions,
    transfer_item_id: Option<Uuid>,
}

impl UploadService {
    /// Starts a resumable upload of `size` bytes from `device`, when the client named one.
    pub async fn create_upload(
        &self,
        owner: UserId,
        device: Option<Uuid>,
        request: UploadRequest<'_>,
    ) -> Result<UploadStatus, FilesError> {
        let UploadRequest {
            target,
            size,
            content_type,
            modified_at,
        } = request;
        if size > self.content.max_upload_bytes {
            return Err(FilesError::TooLarge {
                limit: self.content.max_upload_bytes,
            });
        }
        let place = self.resolve_target(owner, target, size).await?;
        if place.transfer_item_id.is_none() {
            self.content.check_quota(owner, None, size).await?;
        }
        let id = Uuid::now_v7();
        let new = NewUpload {
            id,
            parent_id: place.parent_id,
            node_id: place.node_id,
            name: place.name,
            size: i64::try_from(size).unwrap_or(i64::MAX),
            mime: content_type.map(str::to_owned),
            modified_at,
            device_id: device,
            keep_both: place.options.keep_both,
            version_of: place.options.version_of,
            auto_upload: place.options.auto_upload,
            transfer_item_id: place.transfer_item_id,
        };
        self.open_upload(owner, new).await?;
        Ok(UploadStatus {
            id,
            offset: 0,
            size,
        })
    }

    /// Checks that `target` can take `size` bytes and works out where they will go.
    async fn resolve_target(
        &self,
        owner: UserId,
        target: UploadTarget,
        size: u64,
    ) -> Result<Placement, FilesError> {
        match target {
            UploadTarget::New {
                parent,
                name,
                options,
            } => {
                if let Some(parent) = parent {
                    self.content.db.get_node(owner, parent).await?;
                }
                guard::refuse_invalid_name_of(&self.content.db, owner, parent, &name).await?;
                Ok(Placement {
                    parent_id: parent,
                    name,
                    options,
                    ..Placement::default()
                })
            }
            UploadTarget::Replace(id) => {
                let current = self.content.db.get_node(owner, id).await?;
                if current.kind != NodeKind::File {
                    return Err(FilesError::NotAFile);
                }
                Ok(Placement {
                    node_id: Some(id),
                    name: current.name,
                    ..Placement::default()
                })
            }
            UploadTarget::TransferItem(item_id) => {
                let (transfer, item) = self.content.db.sent_transfer_item(owner, item_id).await?;
                if transfer.state != TransferState::Uploading {
                    return Err(FilesError::Gone);
                }
                if item.kind != NodeKind::File || item.sha256.is_some() {
                    return Err(FilesError::Conflict);
                }
                if u64::try_from(item.size).ok() != Some(size) {
                    return Err(FilesError::InvalidTransfer(format!(
                        "the item has {} bytes, not {size}",
                        item.size
                    )));
                }
                let name = item.path.rsplit('/').next().unwrap_or(&item.path);
                Ok(Placement {
                    name: name.to_owned(),
                    transfer_item_id: Some(item_id),
                    ..Placement::default()
                })
            }
        }
    }

    /// Records the upload `new` and creates its empty file, ready for the first chunk.
    async fn open_upload(&self, owner: UserId, new: NewUpload) -> Result<(), FilesError> {
        let id = new.id;
        let for_transfer = new.transfer_item_id.is_some();
        match self.content.db.create_upload(owner, new).await {
            Ok(_) => {}
            // The transfer's recipient is disabled or marked for deletion.
            Err(db::DbError::NotFound) if for_transfer => return Err(FilesError::Gone),
            Err(err) => return Err(err.into()),
        }
        if let Err(err) = self.content.store.create_partial(id).await {
            self.forget_upload(id).await;
            return Err(err.into());
        }
        self.uploads.insert(id, Some(Hashing::new()));
        Ok(())
    }
}
