import Foundation
import UIKit
import UniformTypeIdentifiers

/// A shared file, copied into this extension's own folder: access to the original ends with the
/// share sheet.
struct SharedFile {
    let name: String
    let url: URL
    let mime: String?
    let size: UInt64
}

/// What another app shared, ready to save or send: files, or text written to a file of its own.
struct SharedContent {
    let files: [SharedFile]
    /// The shared text, when it came as text rather than files.
    let text: String?
    /// The links the text is made of, when it is nothing but links.
    let links: [String]?
    /// Holds the copies; it goes with the share sheet.
    let folder: URL

    var bytes: UInt64 { files.reduce(0) { $0 + $1.size } }

    /// One line on what was shared, for the top of the sheet.
    var summary: String {
        if let links {
            return links.count == 1 ? links[0] : "\(links.count) links"
        }
        if let text {
            let line = text.split(whereSeparator: \.isNewline).first.map(String.init) ?? text
            return "“\(line.trimmingCharacters(in: .whitespaces))”"
        }
        if files.count == 1, let file = files.first {
            return "\(file.name) · \(humanSize(bytes: Int64(file.size)))"
        }
        return sendSummary(files: UInt32(files.count), bytes: bytes)
    }

    /// The SF Symbol for what was shared.
    var symbol: String {
        if links != nil { return "link" }
        if text != nil { return "text.alignleft" }
        if files.allSatisfy({ $0.mime?.hasPrefix("image/") == true }) { return "photo" }
        return "doc"
    }

    var sendFiles: [SendFile] {
        files.map { SendFile(name: $0.name, path: $0.url.path, mime: $0.mime) }
    }

    func remove() {
        try? FileManager.default.removeItem(at: folder)
    }
}

enum SharedContentError: LocalizedError {
    case nothing
    case unreadable(String)

    var errorDescription: String? {
        switch self {
        case .nothing: "There is nothing to save or send."
        case .unreadable(let name): "“\(name)” can't be read."
        }
    }
}

/// Reads what the share sheet hands over, the way the Android app does: files win over the text
/// that comes with them, which is a caption at most; text alone goes as a file named after this
/// phone and the time, or as a list of links when it is only links.
enum SharedContentReader {
    private static let uriListExtension = "uris"

    static func read(_ items: [NSExtensionItem]) async throws -> SharedContent {
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("share", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        do {
            var files: [SharedFile] = []
            var texts: [String] = []
            for provider in items.flatMap({ $0.attachments ?? [] }) {
                if let file = try await file(of: provider, into: folder) {
                    files.append(file)
                } else if let text = await text(of: provider) {
                    texts.append(text)
                }
            }
            if !files.isEmpty {
                return SharedContent(files: files, text: nil, links: nil, folder: folder)
            }
            let text = texts.joined(separator: "\n")
            guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw SharedContentError.nothing
            }
            return try write(text, into: folder)
        } catch {
            try? FileManager.default.removeItem(at: folder)
            throw error
        }
    }

    /// The provider's file, copied into `folder`; `nil` if it holds text or a web link instead.
    private static func file(of provider: NSItemProvider, into folder: URL) async throws -> SharedFile? {
        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
            guard let url = try? await provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier) as? URL
            else { throw SharedContentError.unreadable(provider.suggestedName ?? "File") }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            return try copy(url, name: url.lastPathComponent, type: UTType(filenameExtension: url.pathExtension), into: folder)
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
            return nil
        }
        let types = provider.registeredTypeIdentifiers.compactMap { UTType($0) }
        // Text is a file when it comes with a file name, as from the Files app.
        let named = provider.suggestedName.map { !($0 as NSString).pathExtension.isEmpty } ?? false
        guard let type = types.first(where: { $0.conforms(to: .data) && (named || !$0.conforms(to: .text)) })
        else { return nil }
        return try await withCheckedThrowingContinuation { continuation in
            _ = provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, error in
                // The file goes once this returns, so it is copied here.
                guard let url else {
                    continuation.resume(throwing: SharedContentError.unreadable(provider.suggestedName ?? "File"))
                    return
                }
                var name = url.lastPathComponent
                if let suggested = provider.suggestedName, !suggested.isEmpty {
                    name = suggested
                    if (suggested as NSString).pathExtension.isEmpty, let ext = type.preferredFilenameExtension {
                        name += "." + ext
                    }
                }
                continuation.resume(with: Result { try copy(url, name: name, type: type, into: folder) })
            }
        }
    }

    private static func text(of provider: NSItemProvider) async -> String? {
        if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
            let url = try? await provider.loadItem(forTypeIdentifier: UTType.url.identifier) as? URL
        {
            return url.absoluteString
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
            let item = try? await provider.loadItem(forTypeIdentifier: UTType.plainText.identifier)
            if let text = item as? String { return text }
            if let data = item as? Data { return String(data: data, encoding: .utf8) }
        }
        return nil
    }

    private static func copy(_ source: URL, name: String, type: UTType?, into folder: URL) throws -> SharedFile {
        let name = safeFileName(name, fallback: "File")
        // Each file in a folder of its own, so two of the same name both keep it.
        let own = folder.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: own, withIntermediateDirectories: true)
        let dest = own.appendingPathComponent(name)
        do {
            try FileManager.default.copyItem(at: source, to: dest)
        } catch {
            throw SharedContentError.unreadable(name)
        }
        let size = (try? dest.resourceValues(forKeys: [.fileSizeKey]).fileSize).map { UInt64($0) } ?? 0
        let mime = type?.preferredMIMEType ?? UTType(filenameExtension: dest.pathExtension)?.preferredMIMEType
        return SharedFile(name: name, url: dest, mime: mime, size: size)
    }

    /// Writes `text` to a file named `{device}_{date and time}`, as the Android app does.
    private static func write(_ text: String, into folder: URL) throws -> SharedContent {
        let stamp = DateFormatter()
        stamp.locale = Locale(identifier: "en_US_POSIX")
        stamp.dateFormat = "yyyy-MM-dd_HH-mm-ss"
        // Not a folder, and nothing a computer the file syncs to would refuse.
        let forbidden = CharacterSet(charactersIn: "\\/:*?\"<>|").union(.controlCharacters)
        var from = UIDevice.current.name.components(separatedBy: forbidden).joined(separator: "-")
            .trimmingCharacters(in: .whitespaces)
        if from.isEmpty { from = "iPhone" }
        let links = uriListFromText(text: text)
        let base = "\(from)_\(stamp.string(from: Date()))"
        let name = links != nil ? "\(base).\(uriListExtension)" : base
        let dest = folder.appendingPathComponent(UUID().uuidString)
        try Data((links.map { encodeUriList(uris: $0) } ?? text).utf8).write(to: dest)
        let size = (try? dest.resourceValues(forKeys: [.fileSizeKey]).fileSize).map { UInt64($0) } ?? 0
        let file = SharedFile(name: name, url: dest, mime: links != nil ? "text/uri-list" : "text/plain", size: size)
        return SharedContent(files: [file], text: text, links: links, folder: folder)
    }
}
