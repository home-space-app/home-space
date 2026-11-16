import FileProvider
import OSLog

/// Puts Home Space in Files while someone is signed in, as one location per user, and tells the
/// File Provider extension when the server reports changes: the server has no push to wake it.
enum FileProviderDomains {
    private static let log = Logger(subsystem: "app.homespace.ios", category: "files")

    private static func domain(for user: SignedInUser) -> NSFileProviderDomain {
        NSFileProviderDomain(identifier: NSFileProviderDomainIdentifier("user-\(user.id)"), displayName: "Home Space")
    }

    /// Adds the signed-in user's location, and takes away any other user's.
    static func add(for user: SignedInUser) {
        let wanted = domain(for: user)
        Task {
            do {
                for existing in try await NSFileProviderManager.domains() where existing.identifier != wanted.identifier {
                    try await NSFileProviderManager.remove(existing)
                }
                try await NSFileProviderManager.add(wanted)
            } catch {
                log.error("adding the Files location failed: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    /// Takes the location out of Files, along with the copies the system kept of its files.
    static func removeAll() {
        Task {
            do {
                try await NSFileProviderManager.removeAllDomains()
            } catch {
                log.error("removing the Files location failed: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    /// Asks the extension for the server's latest changes.
    static func signalChanges() {
        Task {
            for domain in (try? await NSFileProviderManager.domains()) ?? [] {
                try? await NSFileProviderManager(for: domain)?.signalEnumerator(for: .workingSet)
            }
        }
    }

    /// The extension has a session again, after it found none or a refused one.
    static func sessionReady() {
        Task {
            for domain in (try? await NSFileProviderManager.domains()) ?? [] {
                try? await NSFileProviderManager(for: domain)?.signalErrorResolved(NSFileProviderError(.notAuthenticated))
            }
        }
    }
}
