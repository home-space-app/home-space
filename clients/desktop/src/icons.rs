use client_core::icon::NodeIcon;
use iced::{
    Color, Element, Theme,
    widget::{svg, svg::Handle},
};

use protocol::Node;

use crate::theme;

/// Default edge length of an icon in logical pixels.
pub const ICON_SIZE: f32 = 20.0;

/// The Material Symbols icons bundled with the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Folder,
    FolderOutline,
    Back,
    /// Waiting for its turn.
    Schedule,
    Laptop,
    Language,
    Smartphone,
    /// The app's mark.
    Home,
    Search,
    MoreVert,
    Image,
    Document,
    Other,
    Upload,
    Download,
    NewFolder,
    Rename,
    Move,
    Delete,
    Restore,
    /// Lost & Found.
    Inventory,
    EmptyBin,
    Refresh,
    Up,
    Close,
    Info,
    Check,
    Down,
    /// A server.
    Dns,
    /// Single sign-on.
    Key,
    /// Shows a password.
    Visibility,
    /// Hides a password.
    VisibilityOff,
    /// The server cannot be reached.
    CloudOff,
    /// Opens what is folded away.
    ExpandMore,
    /// Folds away what is open.
    ExpandLess,
    /// Goes into a folder.
    Chevron,
    /// Sync badge: syncing failed.
    Error,
    More,
    Sync,
    FolderOpen,
    /// Sync badge: in sync.
    Synced,
    /// The sync as a whole has a problem.
    SyncProblem,
    /// Sync badge: set to not sync.
    SyncDisabled,
    /// Opens something in the web browser.
    Web,
    /// Waiting for the sync.
    Update,
    SignOut,
    /// The user's devices.
    Devices,
    /// Pauses the transfers.
    Pause,
    /// Continues paused transfers.
    Resume,
    /// Stops the transfers for good.
    Stop,
    /// Clears finished rows from a list.
    ClearAll,
    /// A device only its owner can send to.
    Lock,
    /// A device everyone can send to.
    Public,
    /// A device some users can send to.
    Group,
    PersonAdd,
    /// Transfers between devices.
    SwapVert,
    /// Saves a transfer on the server.
    SaveToServer,
}

/// How an icon is tinted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tint {
    Text,
    Muted,
    Accent,
    /// A folder that is left out of the sync.
    AccentFaded,
    OnAccent,
    Success,
    Danger,
    Disabled,
}

impl Icon {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Folder => include_bytes!("../assets/icons/folder_fill.svg"),
            Self::FolderOutline => include_bytes!("../assets/icons/folder.svg"),
            Self::Back => include_bytes!("../assets/icons/arrow_back.svg"),
            Self::Schedule => include_bytes!("../assets/icons/schedule.svg"),
            Self::Laptop => include_bytes!("../assets/icons/laptop.svg"),
            Self::Language => include_bytes!("../assets/icons/language.svg"),
            Self::Smartphone => include_bytes!("../assets/icons/smartphone.svg"),
            Self::Home => include_bytes!("../assets/icons/home.svg"),
            Self::Search => include_bytes!("../assets/icons/search.svg"),
            Self::MoreVert => include_bytes!("../assets/icons/more_vert.svg"),
            Self::Image => include_bytes!("../assets/icons/image.svg"),
            Self::Document => include_bytes!("../assets/icons/description.svg"),
            Self::Other => include_bytes!("../assets/icons/draft.svg"),
            Self::Upload => include_bytes!("../assets/icons/upload.svg"),
            Self::Download => include_bytes!("../assets/icons/download.svg"),
            Self::NewFolder => include_bytes!("../assets/icons/create_new_folder.svg"),
            Self::Rename => include_bytes!("../assets/icons/edit.svg"),
            Self::Move => include_bytes!("../assets/icons/drive_file_move.svg"),
            Self::Delete => include_bytes!("../assets/icons/delete.svg"),
            Self::Restore => include_bytes!("../assets/icons/restore_from_trash.svg"),
            Self::Inventory => include_bytes!("../assets/icons/inventory_2.svg"),
            Self::EmptyBin => include_bytes!("../assets/icons/delete_forever.svg"),
            Self::Refresh => include_bytes!("../assets/icons/refresh.svg"),
            Self::Up => include_bytes!("../assets/icons/arrow_upward.svg"),
            Self::Close => include_bytes!("../assets/icons/close.svg"),
            Self::Info => include_bytes!("../assets/icons/info.svg"),
            Self::Check => include_bytes!("../assets/icons/check.svg"),
            Self::Down => include_bytes!("../assets/icons/arrow_downward.svg"),
            Self::Dns => include_bytes!("../assets/icons/dns.svg"),
            Self::Key => include_bytes!("../assets/icons/key.svg"),
            Self::Visibility => include_bytes!("../assets/icons/visibility.svg"),
            Self::VisibilityOff => include_bytes!("../assets/icons/visibility_off.svg"),
            Self::CloudOff => include_bytes!("../assets/icons/cloud_off.svg"),
            Self::ExpandMore => include_bytes!("../assets/icons/expand_more.svg"),
            Self::ExpandLess => include_bytes!("../assets/icons/expand_less.svg"),
            Self::Chevron => include_bytes!("../assets/icons/chevron_right.svg"),
            Self::Error => include_bytes!("../assets/icons/error.svg"),
            Self::More => include_bytes!("../assets/icons/more_horiz.svg"),
            Self::Sync => include_bytes!("../assets/icons/sync.svg"),
            Self::FolderOpen => include_bytes!("../assets/icons/folder_open.svg"),
            Self::Synced => include_bytes!("../assets/icons/check_circle.svg"),
            Self::SyncProblem => include_bytes!("../assets/icons/sync_problem.svg"),
            Self::SyncDisabled => include_bytes!("../assets/icons/sync_disabled.svg"),
            Self::Web => include_bytes!("../assets/icons/open_in_new.svg"),
            Self::Update => include_bytes!("../assets/icons/update.svg"),
            Self::SignOut => include_bytes!("../assets/icons/logout.svg"),
            Self::Devices => include_bytes!("../assets/icons/devices.svg"),
            Self::Pause => include_bytes!("../assets/icons/pause.svg"),
            Self::Resume => include_bytes!("../assets/icons/play_arrow.svg"),
            Self::Stop => include_bytes!("../assets/icons/stop.svg"),
            Self::ClearAll => include_bytes!("../assets/icons/clear_all.svg"),
            Self::Lock => include_bytes!("../assets/icons/lock.svg"),
            Self::Public => include_bytes!("../assets/icons/public.svg"),
            Self::Group => include_bytes!("../assets/icons/group.svg"),
            Self::PersonAdd => include_bytes!("../assets/icons/person_add.svg"),
            Self::SwapVert => include_bytes!("../assets/icons/swap_vert.svg"),
            Self::SaveToServer => include_bytes!("../assets/icons/drive_folder_upload.svg"),
        }
    }

    /// The icon at `size`, tinted from the current theme.
    pub fn view<'a, Message: 'a>(self, size: f32, tint: Tint) -> Element<'a, Message> {
        svg(Handle::from_memory(self.bytes()))
            .width(size)
            .height(size)
            .style(move |theme: &Theme, _status| svg::Style {
                color: Some(tint_color(theme, tint)),
            })
            .into()
    }
}

fn tint_color(theme: &Theme, tint: Tint) -> Color {
    let t = theme::tokens(theme);
    match tint {
        Tint::Text => t.text,
        Tint::Muted => t.muted,
        Tint::Accent => t.accent,
        Tint::AccentFaded => mix(t.surface, t.accent, theme::FADED_ALPHA),
        Tint::OnAccent => t.on_accent,
        Tint::Success => t.success,
        Tint::Danger => t.danger,
        // The svg tint drops alpha, so blend with the background into an opaque colour.
        Tint::Disabled => mix(t.surface, t.text, theme::DISABLED_ALPHA),
    }
}

/// `from` moved `amount` of the way towards `to`.
fn mix(from: Color, to: Color, amount: f32) -> Color {
    let channel = |a: f32, b: f32| a + (b - a) * amount;
    Color::from_rgb(
        channel(from.r, to.r),
        channel(from.g, to.g),
        channel(from.b, to.b),
    )
}

/// The icon that represents `node`.
pub fn node_icon(node: &Node) -> Icon {
    match client_core::icon::node_icon(node) {
        NodeIcon::Folder => Icon::Folder,
        NodeIcon::Image => Icon::Image,
        NodeIcon::Document => Icon::Document,
        NodeIcon::Other => Icon::Other,
    }
}
