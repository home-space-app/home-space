import Observation

/// The tabs of the signed-in app.
enum AppTab: Hashable {
    case files
    case transfers
    case shared
    case settings
}

/// Where the app is, shared with what lives outside the views: a notification tap picks the tab,
/// and notifications stay quiet while the list they would announce is on screen.
@MainActor
@Observable
final class AppRouter {
    static let shared = AppRouter()

    var tab: AppTab = .files
    /// A received transfer to open as soon as the Transfers tab has it, from a notification.
    var openTransfer: String?
    /// Whether the app is in front; set by the app as its scene changes.
    var active = false

    /// Whether the received transfers are on screen, so that a notification would only repeat them.
    var transfersVisible: Bool { active && tab == .transfers }

    private init() {}
}
