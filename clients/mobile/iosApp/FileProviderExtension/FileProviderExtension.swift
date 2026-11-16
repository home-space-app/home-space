import FileProvider
import UniformTypeIdentifiers

/// Home Space as a location in Files and in every app's file picker. The system keeps the local
/// copies and calls in here to fetch or send them; each call runs on its own task.
final class FileProviderExtension: NSObject, NSFileProviderReplicatedExtension {
    private let domain: NSFileProviderDomain
    private let session = ProviderSession()

    required init(domain: NSFileProviderDomain) {
        self.domain = domain
        super.init()
    }

    func invalidate() {}

    func item(
        for identifier: NSFileProviderItemIdentifier, request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        if identifier == .rootContainer {
            completionHandler(ProviderItem.home, nil)
            return Progress()
        }
        if identifier == .trashContainer {
            completionHandler(ProviderItem.bin, nil)
            return Progress()
        }
        guard identifier != .workingSet else {
            completionHandler(nil, NSFileProviderError(.noSuchItem))
            return Progress()
        }
        return run(completion: { completionHandler(nil, $0) }) { client, _ in
            let id = identifier.rawValue
            do {
                completionHandler(ProviderItem(try await client.entry(id: id)), nil)
            } catch CoreError.Failed(let code, _) where code == "not_found" {
                // The server hides binned files from its node lookups.
                guard let binned = try await Self.binned(id, client: client) else { throw NSFileProviderError(.noSuchItem) }
                completionHandler(ProviderItem(binned: binned), nil)
            }
        }
    }

    func fetchContents(
        for itemIdentifier: NSFileProviderItemIdentifier, version requestedVersion: NSFileProviderItemVersion?,
        request: NSFileProviderRequest, completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let domain = domain
        return run(units: 100, completion: { completionHandler(nil, nil, $0) }) { client, progress in
            let id = itemIdentifier.rawValue
            let entry: Entry
            do {
                entry = try await client.entry(id: id)
            } catch CoreError.Failed(let code, _) where code == "not_found" {
                // A binned file: it stays in the bin, but its content can't be read until restored.
                guard try await Self.binned(id, client: client) == nil else { throw CocoaError(.featureUnsupported) }
                throw NSFileProviderError(.noSuchItem)
            }
            // The system moves the file away once it has it.
            let folder = try NSFileProviderManager(for: domain)?.temporaryDirectoryURL()
                ?? FileManager.default.temporaryDirectory
            let dest = folder.appendingPathComponent(UUID().uuidString)
            progress.cancellationHandler = { try? client.cancelDownload(id: id) }
            try await client.saveCopy(id: id, destPath: dest.path, listener: ProgressReporter(progress))
            completionHandler(dest, ProviderItem(entry), nil)
        }
    }

    func createItem(
        basedOn itemTemplate: NSFileProviderItem, fields: NSFileProviderItemFields, contents url: URL?,
        options: NSFileProviderCreateItemOptions = [], request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        let parent = itemTemplate.parentItemIdentifier
        let name = itemTemplate.filename
        let type = itemTemplate.contentType ?? .data
        return run(completion: { completionHandler(nil, [], false, $0) }) { client, _ in
            guard parent != .trashContainer else { throw NSFileProviderError(.noSuchItem) }
            if type.conforms(to: .folder) {
                let folder = try await Self.folder(named: name, in: parent.nodeId, client: client)
                completionHandler(ProviderItem(folder), [], false, nil)
                return
            }
            if type.conforms(to: .symbolicLink) || type.conforms(to: .aliasFile) {
                throw CocoaError(.featureUnsupported)
            }
            let source = try url ?? Self.emptyFile()
            // A new file never takes over one that has the name: it gets the next free name, which
            // Files then shows. Only an item the system says may be there already, as when it
            // rebuilds what it knows, becomes a new version of it.
            let entry = try await client.uploadFile(
                parentId: parent.nodeId, name: name, path: source.path,
                mime: type.preferredMIMEType ?? UTType(filenameExtension: (name as NSString).pathExtension)?.preferredMIMEType,
                keepBoth: !options.contains(.mayAlreadyExist)
            )
            completionHandler(ProviderItem(entry), [], false, nil)
        }
    }

    func modifyItem(
        _ item: NSFileProviderItem, baseVersion version: NSFileProviderItemVersion,
        changedFields: NSFileProviderItemFields, contents newContents: URL?,
        options: NSFileProviderModifyItemOptions = [], request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        let id = item.itemIdentifier.rawValue
        let parent = item.parentItemIdentifier
        let name = item.filename
        let domain = domain
        return run(completion: { completionHandler(nil, [], false, $0) }) { client, _ in
            if changedFields.contains(.parentItemIdentifier) && parent == .trashContainer {
                try await client.delete(ids: [id])
                guard let binned = try await Self.binned(id, client: client) else {
                    // A folder's files go into the bin one by one and the folder itself goes
                    // away; Files learns that when it lists the bin again.
                    Self.signalBin(domain)
                    completionHandler(item, [], false, nil)
                    return
                }
                completionHandler(ProviderItem(binned: binned), [], false, nil)
                return
            }
            if changedFields.contains(.parentItemIdentifier), try await Self.binned(id, client: client) != nil {
                // Put Back, or a move out of the bin: the server restores into Lost & Found only,
                // so the item reports that as its folder.
                let result = try await client.restoreTrashed(ids: [id], mode: .abort)
                if !result.conflicts.isEmpty {
                    throw NSFileProviderError(.filenameCollision)
                }
                Self.signalBin(domain)
            } else if changedFields.contains(.parentItemIdentifier) {
                let moved = try await client.moveNodes(ids: [id], parentId: parent.nodeId, mode: .abort)
                if !moved.conflicts.isEmpty {
                    throw NSFileProviderError(.filenameCollision)
                }
            }
            if changedFields.contains(.filename) {
                _ = try await client.rename(id: id, name: name)
            }
            if changedFields.contains(.contents), let newContents {
                let current = try await client.entry(id: id)
                if ProviderItem(current).itemVersion.contentVersion == version.contentVersion {
                    _ = try await client.replaceFile(id: id, path: newContents.path)
                } else {
                    // The file changed on the server since the edit began, as from another device
                    // while this one was offline. Neither is lost: the edit goes in beside it as
                    // a file of its own, and Files fetches the newer content for this one.
                    _ = try await client.uploadFile(
                        parentId: current.parentId, name: current.name, path: newContents.path,
                        mime: current.mime, keepBoth: true
                    )
                    Self.signalWorkingSet(domain)
                }
            }
            let entry = try await client.entry(id: id)
            completionHandler(ProviderItem(entry), [], false, nil)
        }
    }

    func deleteItem(
        identifier: NSFileProviderItemIdentifier, baseVersion version: NSFileProviderItemVersion,
        options: NSFileProviderDeleteItemOptions = [], request: NSFileProviderRequest,
        completionHandler: @escaping (Error?) -> Void
    ) -> Progress {
        let domain = domain
        return run(completion: completionHandler) { client, _ in
            let id = identifier.rawValue
            if try await Self.binned(id, client: client) != nil {
                // Deleted from the bin: gone for good.
                try await client.deleteForever(ids: [id])
            } else {
                // Files deletes for good only what it doesn't trash; the recycle bin still takes it.
                try await client.delete(ids: [id])
            }
            Self.signalBin(domain)
            completionHandler(nil)
        }
    }

    func enumerator(
        for containerItemIdentifier: NSFileProviderItemIdentifier, request: NSFileProviderRequest
    ) throws -> NSFileProviderEnumerator {
        switch containerItemIdentifier {
        case .workingSet: WorkingSetEnumerator(session: session)
        case .trashContainer: BinEnumerator(session: session)
        default: FolderEnumerator(folder: containerItemIdentifier.nodeId, session: session)
        }
    }

    /// Runs `work` with the signed-in client, and hands any failure to `completion` in the form
    /// Files understands.
    private func run(
        units: Int64 = 1, completion: @escaping (Error) -> Void,
        _ work: @escaping (CoreClient, Progress) async throws -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: units)
        let session = session
        let task = Task {
            do {
                try await session.withClient { try await work($0, progress) }
            } catch {
                completion(providerError(error))
            }
        }
        if progress.cancellationHandler == nil {
            progress.cancellationHandler = { task.cancel() }
        }
        return progress
    }

    /// The folder `name` in `parent`: made, or the one already there, since the system may ask
    /// again for a folder it made before.
    private static func folder(named name: String, in parent: String?, client: CoreClient) async throws -> Entry {
        do {
            return try await client.createFolder(parentId: parent, name: name)
        } catch CoreError.Failed(let code, _) where code == "conflict" {
            if let existing = try await client.list(parentId: parent)
                .first(where: { $0.kind == .folder && $0.name == name })
            {
                return existing
            }
            throw NSFileProviderError(.filenameCollision)
        }
    }

    /// The binned file `id`, or `nil` when it is not in the bin.
    private static func binned(_ id: String, client: CoreClient) async throws -> TrashEntry? {
        try await client.trash().first { $0.id == id }
    }

    /// Has Files ask for the changes on the server.
    private static func signalWorkingSet(_ domain: NSFileProviderDomain) {
        NSFileProviderManager(for: domain)?.signalEnumerator(for: .workingSet) { _ in }
    }

    /// Has Files list the bin again.
    private static func signalBin(_ domain: NSFileProviderDomain) {
        NSFileProviderManager(for: domain)?.signalEnumerator(for: .trashContainer) { _ in }
    }

    private static func emptyFile() throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try Data().write(to: url)
        return url
    }
}

/// Moves a `Progress` along as a download arrives.
private final class ProgressReporter: DownloadListener {
    private let progress: Progress

    init(_ progress: Progress) {
        self.progress = progress
    }

    func progress(id: String, done: Int64, total: Int64) {
        guard total > 0 else { return }
        progress.completedUnitCount = min(progress.totalUnitCount, done * progress.totalUnitCount / total)
    }
}
