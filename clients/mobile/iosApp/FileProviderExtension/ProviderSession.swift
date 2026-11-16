import FileProvider
import Foundation
import OSLog
import UIKit

private let log = Logger(subsystem: "app.homespace.ios.files", category: "provider")

/// Saves the extension's refresh token as soon as Rust reports it: tokens are single use. The
/// server extends the session on each refresh, so the app need not start a new one for a while.
private final class FilesTokenSaver: SessionListener {
    private let server: String
    private let store = SessionStore()

    init(server: String) {
        self.server = server
    }

    func refreshTokenChanged(token: String?) {
        if let token {
            store.saveSession(SavedSession(server: server, refreshToken: token, renewedAt: Date()), for: .files)
        } else {
            store.forgetSession(for: .files)
        }
    }
}

/// The extension's one client, signed in with the session the app started for it
/// (`ExtensionSessions` in the app). Calls arrive on many threads at once; they share it.
actor ProviderSession {
    private var client: CoreClient?
    private var restoring: Task<CoreClient, Error>?

    func client() async throws -> CoreClient {
        if let client { return client }
        if let restoring { return try await restoring.value }
        let task = Task { try await Self.restore() }
        restoring = task
        defer { restoring = nil }
        let restored = try await task.value
        client = restored
        return restored
    }

    /// The server refused the session; the app starts a new one when it next opens.
    func ended() {
        client = nil
        SessionStore().forgetSession(for: .files)
    }

    private static func restore() async throws -> CoreClient {
        let store = SessionStore()
        guard let saved = store.loadSession(for: .files) else {
            store.markNeedsSession(.files)
            throw NSFileProviderError(.notAuthenticated)
        }
        let client = try CoreClient.withSession(
            baseUrl: saved.server,
            device: DeviceIdentity(id: store.deviceId, name: UIDevice.current.name),
            listener: FilesTokenSaver(server: saved.server),
            filesDir: try privateFolder("files").path
        )
        do {
            _ = try await client.restore(refreshToken: saved.refreshToken)
        } catch CoreError.Unauthorized {
            store.forgetSession(for: .files)
            throw NSFileProviderError(.notAuthenticated)
        }
        return client
    }

    /// A folder of the extension's own: a database held open in the App Group's container gets
    /// the extension killed when it is suspended.
    static func privateFolder(_ name: String) throws -> URL {
        var folder = try FileManager.default
            .url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try folder.setResourceValues(values)
        return folder
    }
}

/// What Files makes of a failure: it offers to sign in, says the server can't be reached, and
/// so on.
func providerError(_ error: Error) -> Error {
    switch error {
    case is NSFileProviderError:
        return error
    case CoreError.Unauthorized:
        return NSFileProviderError(.notAuthenticated)
    case CoreError.Unreachable:
        return NSFileProviderError(.serverUnreachable)
    case CoreError.InvalidId:
        return NSFileProviderError(.noSuchItem)
    case CoreError.Cancelled, is CancellationError:
        return CocoaError(.userCancelled)
    case CoreError.Failed(let code, _) where code == "not_found":
        return NSFileProviderError(.noSuchItem)
    case CoreError.Failed(let code, _) where code == "conflict":
        return NSFileProviderError(.filenameCollision)
    case CoreError.Failed(let code, _) where code == "quota_exceeded" || code == "payload_too_large":
        return NSFileProviderError(.insufficientQuota)
    case let error as CocoaError:
        return error
    default:
        // Files accepts only its own errors and Cocoa's; it retries this one later.
        log.error("\(String(describing: error), privacy: .public)")
        return NSFileProviderError(.cannotSynchronize)
    }
}
