import Foundation

/// Keeps a session of its own ready for each of the app's extensions, started from the app's
/// session: the extensions cannot sign in themselves. The server ends them when the app signs
/// out, and one left unused for 30 days expires, so the app starts it again well before.
@MainActor
enum ExtensionSessions {
    /// The extensions that ship so far.
    static let labels: [ExtensionLabel] = [.share, .files]
    /// Started again when this old, as an unused one expires after 30 days.
    private static let renewAfter: TimeInterval = 25 * 24 * 60 * 60
    private static var running: Task<Void, Never>?

    /// Starts the sessions that are missing, refused or old, once at a time.
    static func refresh(client: CoreClient, server: String) {
        guard running == nil else { return }
        running = Task {
            defer { running = nil }
            let store = SessionStore()
            for label in labels where needsNew(label, server: server, store: store) {
                do {
                    let token = try await client.startDeviceSession(label: label.rawValue)
                    store.saveSession(
                        SavedSession(server: server, refreshToken: token, renewedAt: Date()),
                        for: label
                    )
                    if label == .files {
                        FileProviderDomains.sessionReady()
                    }
                } catch {
                    // Tried again the next time the app signs in or opens; an older server
                    // without extension sessions refuses every time.
                    return
                }
            }
        }
    }

    /// Forgets the extensions' sessions; the server ends them with the app's own.
    static func forget() {
        running?.cancel()
        running = nil
        SessionStore().forgetExtensionSessions()
    }

    private static func needsNew(_ label: ExtensionLabel, server: String, store: SessionStore) -> Bool {
        guard !store.needsSession(label), let saved = store.loadSession(for: label),
            saved.server == server, let renewed = saved.renewedAt
        else { return true }
        return Date().timeIntervalSince(renewed) > renewAfter
    }
}
