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
        guard identifier != .trashContainer, identifier != .workingSet else {
            completionHandler(nil, NSFileProviderError(.noSuchItem))
            return Progress()
        }
        return run(completion: { completionHandler(nil, $0) }) { client, _ in
            let entry = try await client.entry(id: identifier.rawValue)
            completionHandler(ProviderItem(entry), nil)
        }
    }

    func fetchContents(
        for itemIdentifier: NSFileProviderItemIdentifier, version requestedVersion: NSFileProviderItemVersion?,
        request: NSFileProviderRequest, completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let domain = domain
        return run(units: 100, completion: { completionHandler(nil, nil, $0) }) { client, progress in
            let id = itemIdentifier.rawValue
            let entry = try await client.entry(id: id)
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
            // A name that is taken gets a new version, as the app's uploads do.
            let entry = try await client.uploadFile(
                parentId: parent.nodeId, name: name, path: source.path,
                mime: type.preferredMIMEType ?? UTType(filenameExtension: (name as NSString).pathExtension)?.preferredMIMEType
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
        return run(completion: { completionHandler(nil, [], false, $0) }) { client, _ in
            if changedFields.contains(.parentItemIdentifier) {
                guard parent != .trashContainer else {
                    // Files has no trash of ours to show; the recycle bin takes it.
                    try await client.delete(ids: [id])
                    completionHandler(nil, [], false, nil)
                    return
                }
                let moved = try await client.moveNodes(ids: [id], parentId: parent.nodeId, mode: .abort)
                if !moved.conflicts.isEmpty {
                    throw NSFileProviderError(.filenameCollision)
                }
            }
            if changedFields.contains(.filename) {
                _ = try await client.rename(id: id, name: name)
            }
            if changedFields.contains(.contents), let newContents {
                _ = try await client.replaceFile(id: id, path: newContents.path)
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
        run(completion: completionHandler) { client, _ in
            // Into the recycle bin, as everywhere else, rather than gone for good.
            try await client.delete(ids: [identifier.rawValue])
            completionHandler(nil)
        }
    }

    func enumerator(
        for containerItemIdentifier: NSFileProviderItemIdentifier, request: NSFileProviderRequest
    ) throws -> NSFileProviderEnumerator {
        switch containerItemIdentifier {
        case .workingSet: WorkingSetEnumerator(session: session)
        case .trashContainer: EmptyEnumerator()
        default: FolderEnumerator(folder: containerItemIdentifier.nodeId, session: session)
        }
    }

    /// Runs `work` with the signed-in client, and hands any failure to `completion` in the form
    /// Files understands. A refused session is forgotten, so the app starts another.
    private func run(
        units: Int64 = 1, completion: @escaping (Error) -> Void,
        _ work: @escaping (CoreClient, Progress) async throws -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: units)
        let session = session
        let task = Task {
            do {
                try await work(session.client(), progress)
            } catch {
                if case CoreError.Unauthorized = error { await session.ended() }
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
