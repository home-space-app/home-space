import Foundation
import Observation
import UIKit

/// Saves the refresh token as soon as Rust reports it: tokens are single use, so one that was
/// rotated but not saved would sign the user out on the next start.
private final class TokenSaver: SessionListener {
    private let server: String
    private let store: SessionStore

    init(server: String, store: SessionStore) {
        self.server = server
        self.store = store
    }

    func refreshTokenChanged(token: String?) {
        if let token {
            store.saveSession(SavedSession(server: server, refreshToken: token))
        } else {
            store.forgetSession()
        }
    }
}

/// Tells the screens that what auto-upload reports may have changed.
@MainActor
@Observable
final class SyncChanges {
    private(set) var generation = 0

    func changed() {
        generation &+= 1
    }
}

private final class ChangeForwarder: AutoUploadListener {
    private let changes: SyncChanges

    init(changes: SyncChanges) {
        self.changes = changes
    }

    func changed() {
        let changes = changes
        Task { @MainActor in changes.changed() }
    }
}

/// Tells the session's screens that the upload queue may have changed.
private final class UploadsForwarder: AutoUploadListener {
    func changed() {
        Task { @MainActor in ClientHolder.shared.onUploadsChanged?() }
    }
}

/// The app copies a picked file itself when adding it, so the queue never asks.
private final class NoPickedSources: PickedFiles {
    func copyFile(source: String, dest: String) throws {
        throw CoreError.Failed(code: "file", detail: "this file can't be read")
    }

    func release(source: String) {}
}

/// The signed-in client and what hangs off it.
@MainActor
final class ActiveSession {
    let client: CoreClient
    let user: SignedInUser
    private let changes: SyncChanges
    private var opened: AutoUpload?
    private var openedQueue: UploadQueue?

    init(client: CoreClient, user: SignedInUser, changes: SyncChanges) {
        self.client = client
        self.user = user
        self.changes = changes
        client.setOfflineListener(listener: ChangeForwarder(changes: changes))
    }

    /// Opened once in this run, so that it can tell what the app ended in the middle of.
    func uploadQueue() throws -> UploadQueue {
        if let openedQueue {
            return openedQueue
        }
        let queue = try UploadQueue.open(
            client: client,
            dataDir: try ClientHolder.syncFolder().path,
            files: NoPickedSources(),
            listener: UploadsForwarder()
        )
        openedQueue = queue
        return queue
    }

    func autoUpload() throws -> AutoUpload {
        if let opened {
            return opened
        }
        let auto = try AutoUpload.open(
            client: client,
            dataDir: try ClientHolder.syncFolder().path,
            local: IOSSources(),
            listener: ChangeForwarder(changes: changes)
        )
        opened = auto
        return auto
    }
}

/// The one client of this process, shared by the screens and background tasks.
///
/// Refresh tokens are single use and replaying one ends the session, so two clients must never
/// restore from the same saved token: whichever comes second, the app or a background task, has
/// to find the first one here.
@MainActor
final class ClientHolder {
    static let shared = ClientHolder()

    let changes = SyncChanges()
    /// Called when the upload queue may have changed.
    var onUploadsChanged: (() -> Void)?
    private(set) var active: ActiveSession?
    private let store = SessionStore()
    private var restoring: Task<ActiveSession?, Error>?

    private init() {}

    func newClient(server: String) throws -> CoreClient {
        Self.removeLegacyDownloads()
        return try CoreClient.withSession(
            baseUrl: server,
            device: DeviceIdentity(id: store.deviceId, name: UIDevice.current.name),
            listener: TokenSaver(server: server, store: store),
            filesDir: try Self.offlineFolder().path
        )
    }

    /// Makes a client that has just signed in the active one.
    func adopt(client: CoreClient, user: SignedInUser) -> ActiveSession {
        let session = ActiveSession(client: client, user: user, changes: changes)
        active = session
        return session
    }

    /// The active session, else one restored from the saved token, else `nil` when nobody is
    /// signed in. A token the server refused is forgotten.
    func resume() async throws -> ActiveSession? {
        if let active {
            return active
        }
        if let restoring {
            return try await restoring.value
        }
        let task = Task<ActiveSession?, Error> { @MainActor in
            guard let saved = store.loadSession() else { return nil }
            do {
                let client = try newClient(server: saved.server)
                let user = try await client.restore(refreshToken: saved.refreshToken)
                return adopt(client: client, user: user)
            } catch CoreError.Unauthorized {
                store.forgetSession()
                throw CoreError.Unauthorized
            }
        }
        restoring = task
        defer { restoring = nil }
        return try await task.value
    }

    /// The session ended.
    func release() {
        active = nil
    }

    /// Where offline copies are kept. They can be downloaded again, so they stay out of backups.
    private static func offlineFolder() throws -> URL {
        try privateFolder("offline")
    }

    /// Copies from before they were synced. They download again when opened.
    private static func removeLegacyDownloads() {
        guard
            let support = try? FileManager.default.url(
                for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: false)
        else { return }
        try? FileManager.default.removeItem(at: support.appendingPathComponent("downloads", isDirectory: true))
    }

    /// What auto-upload remembers. It describes this phone's files, so it stays out of backups.
    static func syncFolder() throws -> URL {
        try privateFolder("sync")
    }
}
