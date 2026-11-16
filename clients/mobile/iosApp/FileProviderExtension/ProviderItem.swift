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

/// A Home Space file or folder as Files sees it, or a file in the recycle bin. Ids stay the same
/// across moves and renames, and into the bin and back.
final class ProviderItem: NSObject, NSFileProviderItem {
    private enum Backing {
        case home
        case bin
        case entry(Entry)
        case binned(TrashEntry)
    }

    private let backing: Backing

    init(_ entry: Entry) {
        backing = .entry(entry)
    }

    init(binned entry: TrashEntry) {
        backing = .binned(entry)
    }

    private init(_ backing: Backing) {
        self.backing = backing
    }

    static let home = ProviderItem(.home)

    /// Files' trash, which is the server's recycle bin.
    static let bin = ProviderItem(.bin)

    /// Whether Files should never see `entry`.
    static func isHidden(_ entry: Entry) -> Bool {
        entry.parentId == nil && entry.kind == .folder && entry.name.hasPrefix(binPrefix)
    }

    private var entry: Entry? {
        if case .entry(let entry) = backing { return entry }
        return nil
    }

    var itemIdentifier: NSFileProviderItemIdentifier {
        switch backing {
        case .home: .rootContainer
        case .bin: .trashContainer
        case .entry(let entry): NSFileProviderItemIdentifier(entry.id)
        case .binned(let entry): NSFileProviderItemIdentifier(entry.id)
        }
    }

    var parentItemIdentifier: NSFileProviderItemIdentifier {
        switch backing {
        case .home, .bin: .rootContainer
        case .entry(let entry): NSFileProviderItemIdentifier(node: entry.parentId)
        case .binned: .trashContainer
        }
    }

    var filename: String {
        switch backing {
        case .home: "Home Space"
        case .bin: "Recycle Bin"
        case .entry(let entry): entry.name
        case .binned(let entry): entry.name
        }
    }

    var contentType: UTType {
        switch backing {
        case .home, .bin: return .folder
        case .entry(let entry):
            guard entry.kind == .file else { return .folder }
            return Self.type(mime: entry.mime, name: entry.name)
        case .binned(let entry):
            return Self.type(mime: entry.mime, name: entry.name)
        }
    }

    var documentSize: NSNumber? {
        switch backing {
        case .entry(let entry) where entry.kind == .file: NSNumber(value: entry.size)
        case .binned(let entry): NSNumber(value: entry.size)
        default: nil
        }
    }

    var contentModificationDate: Date? {
        switch backing {
        case .entry(let entry): entry.updatedAt
        case .binned(let entry): entry.deletedAt
        default: nil
        }
    }

    var capabilities: NSFileProviderItemCapabilities {
        switch backing {
        case .home:
            return [.allowsReading, .allowsContentEnumerating, .allowsAddingSubItems]
        case .bin:
            return [.allowsReading, .allowsContentEnumerating]
        case .entry(let entry) where entry.kind == .folder:
            return [
                .allowsReading, .allowsContentEnumerating, .allowsAddingSubItems,
                .allowsRenaming, .allowsReparenting, .allowsTrashing, .allowsDeleting,
            ]
        case .entry:
            return [.allowsReading, .allowsWriting, .allowsRenaming, .allowsReparenting, .allowsTrashing, .allowsDeleting]
        case .binned:
            // Files offers Recover only for readable items. The server can't send a binned
            // file's content, so opening one fails; it can be put back or deleted for good.
            return [.allowsReading, .allowsReparenting, .allowsDeleting]
        }
    }

    /// The content changes with the hash, or the version where the server has no hash; the rest
    /// with the name, the folder or the time of the last change.
    var itemVersion: NSFileProviderItemVersion {
        switch backing {
        case .home, .bin:
            let fixed = Data(filename.utf8)
            return NSFileProviderItemVersion(contentVersion: fixed, metadataVersion: fixed)
        case .entry(let entry):
            let content = entry.sha256 ?? "v\(entry.version)"
            let metadata = "\(entry.name)|\(entry.parentId ?? "")|\(entry.updatedAt.timeIntervalSince1970)"
            return NSFileProviderItemVersion(contentVersion: Data(content.utf8), metadataVersion: Data(metadata.utf8))
        case .binned(let entry):
            let metadata = "\(entry.name)|bin|\(entry.deletedAt.timeIntervalSince1970)"
            return NSFileProviderItemVersion(contentVersion: Data("bin".utf8), metadataVersion: Data(metadata.utf8))
        }
    }

    private static func type(mime: String, name: String) -> UTType {
        UTType(mimeType: mime, conformingTo: .data)
            ?? UTType(filenameExtension: (name as NSString).pathExtension)
            ?? .data
    }
}
