//! Wire types shared by the home-space server and its clients.

mod admin;
mod auth;
mod capabilities;
mod change;
pub mod compat;
mod device;
mod duplicates;
pub mod encoding;
mod error;
mod event;
pub mod features;
mod health;
mod ids;
mod node;
pub mod patch;
pub mod paths;
mod problem;
mod problem_type;
pub mod rules;
mod search;
mod share;
mod storage;
mod ticket;
mod transfer;
mod trash;
mod upload;
mod validate;
pub mod version;

pub use admin::{Account, AccountDevice, CreateAccount, SetPassword, UpdateAccount};
pub use auth::{
    ChangePasswordRequest, DEVICE_SESSION_LABELS, DeviceSessionRequest, LoginRequest, Me,
    OidcCallbackQuery, OidcExchangeRequest, OidcProvider, OidcStartQuery, Providers,
    RefreshRequest, TokenPair,
};
pub use capabilities::Capabilities;
pub use change::{Change, ChangesQuery, ChangesResponse};
pub use compat::{Compatibility, Incompatibility, Newer};
pub use device::{
    Device, DeviceInfo, DeviceVisibility, PLATFORM_ANDROID, PLATFORM_IOS, PLATFORM_LINUX,
    PLATFORM_MACOS, PLATFORM_WEB, PLATFORM_WINDOWS, RenameDevice, SetDeviceVisibility,
    TargetDevice, TransferTarget, UserRef,
};
pub use duplicates::{DuplicateFile, DuplicateGroup, DuplicatesReport, OutdatedMatch};
pub use error::ErrorCode;
pub use event::{ClientMessage, Event};
pub use health::Health;
pub use ids::{NodeId, UserId};
pub use node::{
    Conflict, ContentQuery, CreateFolderRequest, FileVersion, ListQuery, MoveRequest, MoveResponse,
    Node, NodeKind, NodeProperties, OnConflict, UpdateRequest, UploadQuery,
};
pub use patch::Patch;
pub use problem::{AsProblem, FieldError, PROBLEM_CONTENT_TYPE, Problem};
pub use problem_type::{PROBLEMS_BASE, ProblemType};
pub use search::{SearchHit, SearchQuery};
pub use share::{
    CopyShared, CreateShare, PublicShare, ReceivedShare, Share, ShareAccess, ShareAccessQuery,
    ShareArchiveQuery, SharePermission, ShareRecipient, SharedFolderQuery, SharedNode, SharesQuery,
    UnlockShare, UpdateShare,
};
pub use storage::{
    DatabaseStatus, DiskSpace, DiskStatus, OidcStatus, ServerConfig, ServerStatus, Storage,
    StorageBackend, StorageUsage, UserStorage,
};
pub use ticket::{TicketQuery, TicketRequest, TicketResponse};
pub use transfer::{
    CreateTransfer, CreatedTransfer, CreatedTransferItem, IncomingQuery, NewTransferItem,
    SaveTransfer, Transfer, TransferItem, TransferState, TransferSummary,
};
pub use trash::{RestoreRequest, TrashItem};
pub use upload::{ChunkResult, CreateUpload, UploadInfo, UploadSession, UploadsQuery};
pub use validate::Validate;
pub use version::Version;
