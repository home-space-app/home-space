import Foundation

/// Keeps an extension's refresh token in the Keychain as Rust reports it: tokens are single use.
/// The server extends the session on each refresh, so the app need not start a new one for a
/// while.
///
/// The app may start a new session for the extension at any time (`ExtensionSessions`), which
/// ends the one this client holds. The refusal that follows forgets the saved token only while
/// it is still this client's own, so the new one survives for the next client to use.
final class ExtensionTokenSaver: SessionListener, @unchecked Sendable {
    private let server: String
    private let label: ExtensionLabel
    private let store = SessionStore()
    private let lock = NSLock()
    /// The token this client holds: the one it signed in with, then each it was given.
    private var current: String

    init(server: String, label: ExtensionLabel, token: String) {
        self.server = server
        self.label = label
        current = token
    }

    func refreshTokenChanged(token: String?) {
        guard let token else {
            sessionEnded()
            return
        }
        lock.withLock {
            current = token
            store.saveSession(SavedSession(server: server, refreshToken: token, renewedAt: Date()), for: label)
        }
    }

    /// The server refused this client's session. The saved one goes, and the app starts
    /// another when it next opens, unless the app has saved a newer one meanwhile.
    func sessionEnded() {
        lock.withLock {
            guard store.loadSession(for: label)?.refreshToken == current else { return }
            store.forgetSession(for: label)
        }
    }
}

/// A folder of this process's own, not the App Group's, kept out of backups: a database held
/// open in a shared container gets an extension killed when it is suspended.
func privateFolder(_ name: String) throws -> URL {
    var folder = try FileManager.default
        .url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        .appendingPathComponent(name, isDirectory: true)
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try folder.setResourceValues(values)
    return folder
}
