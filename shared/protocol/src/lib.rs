//! Wire types shared by the home-space server and its clients.

mod admin;
mod auth;
mod capabilities;
mod change;
mod device;
mod error;
mod event;
mod health;
mod ids;
mod node;
pub mod paths;
mod search;
mod storage;
mod ticket;
mod transfer;
mod trash;
mod upload;

pub use admin::{Account, AccountDevice, CreateAccount, SetPassword, UpdateAccount};
pub use auth::{
    ChangePasswordRequest, LoginRequest, Me, OidcCallbackQuery, OidcExchangeRequest, OidcProvider,
    OidcStartQuery, Providers, RefreshRequest, TokenPair,
};
pub use capabilities::Capabilities;
pub use change::{Change, ChangesQuery, ChangesResponse};
pub use device::{
    Device, DeviceInfo, DeviceVisibility, PLATFORM_ANDROID, PLATFORM_IOS, PLATFORM_LINUX,
    PLATFORM_MACOS, PLATFORM_WEB, PLATFORM_WINDOWS, RenameDevice, SetDeviceVisibility,
    TargetDevice, TransferTarget, UserRef,
};
pub use error::{ErrorBody, ErrorCode};
pub use event::{ClientMessage, Event};
pub use health::Health;
pub use ids::{NodeId, UserId};
pub use node::{
    Conflict, ContentQuery, CreateFolderRequest, FileVersion, ListQuery, MoveRequest, MoveResponse,
    Node, NodeKind, NodeProperties, OnConflict, UpdateRequest, UploadQuery,
};
pub use search::{SearchHit, SearchQuery};
pub use storage::{
    DatabaseStatus, DiskSpace, DiskStatus, OidcStatus, ServerConfig, ServerStatus, Storage,
    StorageBackend, StorageUsage, UserStorage,
};
pub use ticket::{TicketRequest, TicketResponse};
pub use transfer::{
    CreateTransfer, CreatedTransfer, CreatedTransferItem, IncomingQuery, NewTransferItem,
    SaveTransfer, Transfer, TransferItem, TransferState, TransferSummary,
};
pub use trash::{RestoreRequest, TrashItem};
pub use upload::{ChunkResult, CreateUpload, UploadInfo, UploadSession, UploadsQuery};
