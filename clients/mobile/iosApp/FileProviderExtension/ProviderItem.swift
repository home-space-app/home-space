import FileProvider
import UniformTypeIdentifiers

/// The folder of the user's recycle bin, which Home lists like any other.
private let binPrefix = ".recycle-bin-"

extension NSFileProviderItemIdentifier {
    /// The node id of a folder, `nil` for Home.
    var nodeId: String? {
        self == .rootContainer ? nil : rawValue
    }

    init(node id: String?) {
        self = id.map { NSFileProviderItemIdentifier($0) } ?? .rootContainer
    }
}

/// A Home Space file or folder as Files sees it. Ids stay the same across moves and renames.
final class ProviderItem: NSObject, NSFileProviderItem {
    private let entry: Entry?

    init(_ entry: Entry) {
        self.entry = entry
    }

    /// Home.
    private override init() {
        entry = nil
    }

    static let home = ProviderItem()

    /// Whether Files should never see `entry`.
    static func isHidden(_ entry: Entry) -> Bool {
        entry.parentId == nil && entry.kind == .folder && entry.name.hasPrefix(binPrefix)
    }

    var itemIdentifier: NSFileProviderItemIdentifier {
        entry.map { NSFileProviderItemIdentifier($0.id) } ?? .rootContainer
    }

    var parentItemIdentifier: NSFileProviderItemIdentifier {
        NSFileProviderItemIdentifier(node: entry?.parentId)
    }

    var filename: String {
        entry?.name ?? "Home Space"
    }

    var contentType: UTType {
        guard let entry, entry.kind == .file else { return .folder }
        return UTType(mimeType: entry.mime, conformingTo: .data)
            ?? UTType(filenameExtension: (entry.name as NSString).pathExtension)
            ?? .data
    }

    var documentSize: NSNumber? {
        guard let entry, entry.kind == .file else { return nil }
        return NSNumber(value: entry.size)
    }

    var contentModificationDate: Date? {
        entry?.updatedAt
    }

    var capabilities: NSFileProviderItemCapabilities {
        guard let entry else { return [.allowsReading, .allowsContentEnumerating, .allowsAddingSubItems] }
        if entry.kind == .folder {
            return [
                .allowsReading, .allowsContentEnumerating, .allowsAddingSubItems,
                .allowsRenaming, .allowsReparenting, .allowsDeleting,
            ]
        }
        return [.allowsReading, .allowsWriting, .allowsRenaming, .allowsReparenting, .allowsDeleting]
    }

    /// The content changes with the hash, or the version where the server has no hash; the rest
    /// with the name, the folder or the time of the last change.
    var itemVersion: NSFileProviderItemVersion {
        guard let entry else {
            return NSFileProviderItemVersion(contentVersion: Data("home".utf8), metadataVersion: Data("home".utf8))
        }
        let content = entry.sha256 ?? "v\(entry.version)"
        let metadata = "\(entry.name)|\(entry.parentId ?? "")|\(entry.updatedAt.timeIntervalSince1970)"
        return NSFileProviderItemVersion(contentVersion: Data(content.utf8), metadataVersion: Data(metadata.utf8))
    }
}
