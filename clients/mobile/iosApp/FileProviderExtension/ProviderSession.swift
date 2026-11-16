import FileProvider
import Foundation
import OSLog
import UIKit

private let log = Logger(subsystem: "app.homespace.ios.files", category: "provider")

/// The extension's one client, signed in with the session the app started for it
/// (`ExtensionSessions` in the app). Calls arrive on many threads at once; they share it.
actor ProviderSession {
    private typealias SignedIn = (client: CoreClient, saver: ExtensionTokenSaver)

    private var signedIn: SignedIn?
    private var restoring: Task<SignedIn, Error>?

    /// Runs `work` with the signed-in client. When the server refuses its session, the next
    /// call signs in again with whatever session is saved by then: the app may have started
    /// a new one, which is what ended this one.
    nonisolated func withClient<T>(_ work: (CoreClient) async throws -> T) async throws -> T {
        let current = try await signIn()
        do {
            return try await work(current.client)
        } catch {
            if case CoreError.Unauthorized = error {
                current.saver.sessionEnded()
                await ended(current.client)
            }
            throw error
        }
    }

    private func signIn() async throws -> SignedIn {
        if let signedIn { return signedIn }
        if let restoring { return try await restoring.value }
        let task = Task { try await Self.restore() }
        restoring = task
        defer { restoring = nil }
        let restored = try await task.value
        signedIn = restored
        return restored
    }

    /// Drops `client`, unless another has replaced it already.
    private func ended(_ client: CoreClient) {
        if signedIn?.client === client {
            signedIn = nil
        }
    }

    private static func restore() async throws -> SignedIn {
        let store = SessionStore()
        guard let saved = store.loadSession(for: .files) else {
            store.markNeedsSession(.files)
            throw NSFileProviderError(.notAuthenticated)
        }
        let saver = ExtensionTokenSaver(server: saved.server, label: .files, token: saved.refreshToken)
        let client = try CoreClient.withSession(
            baseUrl: saved.server,
            device: DeviceIdentity(id: store.deviceId, name: UIDevice.current.name),
            listener: saver,
            filesDir: try privateFolder("files").path
        )
        do {
            _ = try await client.restore(refreshToken: saved.refreshToken)
        } catch CoreError.Unauthorized {
            saver.sessionEnded()
            throw NSFileProviderError(.notAuthenticated)
        }
        return (client, saver)
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
