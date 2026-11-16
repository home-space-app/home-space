import CryptoKit
import FileProvider

/// Lists one folder, Home included. Changes come through the working set, so a folder reports
/// none of its own.
final class FolderEnumerator: NSObject, NSFileProviderEnumerator {
    private let folder: String?
    private let session: ProviderSession
    private var task: Task<Void, Never>?

    init(folder: String?, session: ProviderSession) {
        self.folder = folder
        self.session = session
    }

    func invalidate() {
        task?.cancel()
    }

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        let folder = folder
        let session = session
        task = Task {
            do {
                let entries = try await session.client().list(parentId: folder)
                observer.didEnumerate(entries.filter { !ProviderItem.isHidden($0) }.map(ProviderItem.init))
                observer.finishEnumerating(upTo: nil)
            } catch {
                if case CoreError.Unauthorized = error { await session.ended() }
                observer.finishEnumeratingWithError(providerError(error))
            }
        }
    }

    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        observer.finishEnumeratingChanges(upTo: anchor, moreComing: false)
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        completionHandler(NSFileProviderSyncAnchor(Data("folder".utf8)))
    }
}

/// Every change on the server, from the server's change feed; its cursor is the sync anchor.
/// The system keeps the anchor, so the extension needs no database. The set itself starts empty:
/// Files learns of items as the user opens folders.
final class WorkingSetEnumerator: NSObject, NSFileProviderEnumerator {
    private let session: ProviderSession
    private var task: Task<Void, Never>?

    init(session: ProviderSession) {
        self.session = session
    }

    func invalidate() {
        task?.cancel()
    }

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        observer.finishEnumerating(upTo: nil)
    }

    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        guard let since = Self.cursor(anchor) else {
            observer.finishEnumeratingWithError(NSFileProviderError(.syncAnchorExpired))
            return
        }
        let session = session
        task = Task {
            do {
                let page = try await session.client().changes(since: since)
                var updated: [NSFileProviderItem] = []
                var deleted: [NSFileProviderItemIdentifier] = []
                for change in page.changes {
                    switch change {
                    case .upserted(let entry) where !ProviderItem.isHidden(entry):
                        updated.append(ProviderItem(entry))
                    case .upserted:
                        break
                    case .deleted(let id):
                        deleted.append(NSFileProviderItemIdentifier(id))
                    }
                }
                if !deleted.isEmpty {
                    // The feed reads a binned file as deleted; Files should see it in its trash.
                    let binned = try await session.client().trash()
                    let ids = Set(deleted.map(\.rawValue))
                    let moved = binned.filter { ids.contains($0.id) }
                    updated += moved.map { ProviderItem(binned: $0) }
                    let movedIds = Set(moved.map(\.id))
                    deleted.removeAll { movedIds.contains($0.rawValue) }
                }
                if !updated.isEmpty { observer.didUpdate(updated) }
                if !deleted.isEmpty { observer.didDeleteItems(withIdentifiers: deleted) }
                observer.finishEnumeratingChanges(upTo: Self.anchor(page.cursor), moreComing: page.more)
            } catch CoreError.Failed(let code, _) where code == "gone" {
                // The server has pruned the changes since then; the system lists everything again.
                observer.finishEnumeratingWithError(NSFileProviderError(.syncAnchorExpired))
            } catch {
                if case CoreError.Unauthorized = error { await session.ended() }
                observer.finishEnumeratingWithError(providerError(error))
            }
        }
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        let session = session
        Task {
            let page = try? await session.client().changes(since: nil)
            completionHandler(page.map { Self.anchor($0.cursor) })
        }
    }

    private static func anchor(_ cursor: Int64) -> NSFileProviderSyncAnchor {
        NSFileProviderSyncAnchor(Data(String(cursor).utf8))
    }

    private static func cursor(_ anchor: NSFileProviderSyncAnchor) -> Int64? {
        Int64(String(decoding: anchor.rawValue, as: UTF8.self))
    }
}

/// Files' trash: the files in the server's recycle bin. The anchor is a digest of what the bin
/// holds, so any change has the system list it again.
final class BinEnumerator: NSObject, NSFileProviderEnumerator {
    private let session: ProviderSession
    private var task: Task<Void, Never>?

    init(session: ProviderSession) {
        self.session = session
    }

    func invalidate() {
        task?.cancel()
    }

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        let session = session
        task = Task {
            do {
                let binned = try await session.client().trash()
                observer.didEnumerate(binned.map { ProviderItem(binned: $0) })
                observer.finishEnumerating(upTo: nil)
            } catch {
                if case CoreError.Unauthorized = error { await session.ended() }
                observer.finishEnumeratingWithError(providerError(error))
            }
        }
    }

    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        let session = session
        task = Task {
            do {
                let current = Self.anchor(try await session.client().trash())
                if current == anchor {
                    observer.finishEnumeratingChanges(upTo: anchor, moreComing: false)
                } else {
                    // The bin has no change feed; listing it again is cheap.
                    observer.finishEnumeratingWithError(NSFileProviderError(.syncAnchorExpired))
                }
            } catch {
                if case CoreError.Unauthorized = error { await session.ended() }
                observer.finishEnumeratingWithError(providerError(error))
            }
        }
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        let session = session
        Task {
            let binned = try? await session.client().trash()
            completionHandler(binned.map(Self.anchor))
        }
    }

    private static func anchor(_ binned: [TrashEntry]) -> NSFileProviderSyncAnchor {
        let listing = binned.map { "\($0.id)@\($0.deletedAt.timeIntervalSince1970)" }.joined(separator: ",")
        return NSFileProviderSyncAnchor(Data(SHA256.hash(data: Data(listing.utf8))))
    }
}
