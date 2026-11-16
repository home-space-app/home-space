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
    /// Moves on as the extensions' sessions are forgotten, so a run started before then saves
    /// nothing: the server has ended what it started.
    private static var generation = 0

    /// Starts the sessions that are missing, refused or old, one at a time.
    static func refresh(client: CoreClient, server: String) {
        guard running == nil else { return }
        let started = generation
        running = Task {
            defer { if generation == started { running = nil } }
            let store = SessionStore()
            for label in labels where needsNew(label, server: server, store: store) {
                let token: String
                do {
                    token = try await client.startDeviceSession(label: label.rawValue)
                } catch {
                    // Tried again the next time the app signs in or opens; an older server
                    // without extension sessions refuses every time. The other labels still
                    // get theirs.
                    continue
                }
                guard generation == started, !Task.isCancelled else { return }
                store.saveSession(SavedSession(server: server, refreshToken: token, renewedAt: Date()), for: label)
                if label == .files {
                    FileProviderDomains.sessionReady()
                }
            }
        }
    }

    /// Forgets the extensions' sessions; the server ends them with the app's own.
    static func forget() {
        generation += 1
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
