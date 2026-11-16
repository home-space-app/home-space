import Foundation
import SwiftUI
import UIKit

/// Forwards progress reports to the main actor; they come from the Rust runtime's threads.
final class ProgressForwarder: DownloadListener {
    private let report: @Sendable (String, Double) -> Void

    /// `onProgress` gets the id the report is about, and how far it is, from 0 to 1.
    init(_ onProgress: @escaping @MainActor (String, Double) -> Void) {
        report = { id, fraction in
            Task { @MainActor in onProgress(id, fraction) }
        }
    }

    func progress(id: String, done: Int64, total: Int64) {
        report(id, total > 0 ? min(Double(done) / Double(total), 1) : 1)
    }
}

/// A photo or video from the library, as a file of its own. The file the system hands over goes
/// away once this returns.
struct PickedPhoto: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .image) { try PickedPhoto(keeping: $0) }
        FileRepresentation(importedContentType: .movie) { try PickedPhoto(keeping: $0) }
    }

    private init(keeping received: ReceivedTransferredFile) throws {
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        url = folder.appendingPathComponent(received.file.lastPathComponent)
        try FileManager.default.copyItem(at: received.file, to: url)
    }
}

/// Time to finish after the user leaves the app. `onExpiry` runs when iOS is about to end it.
@MainActor
final class BackgroundTime {
    private var task = UIBackgroundTaskIdentifier.invalid

    init(name: String, onExpiry: @escaping @MainActor () -> Void) {
        task = UIApplication.shared.beginBackgroundTask(withName: name) { [weak self] in
            MainActor.assumeIsolated {
                onExpiry()
                self?.end()
            }
        }
    }

    func end() {
        if task != .invalid {
            UIApplication.shared.endBackgroundTask(task)
            task = .invalid
        }
    }
}

/// A file name that stays inside the folder it is written to, whatever the server sent.
func safeFileName(_ name: String, fallback: String) -> String {
    let last = (name as NSString).lastPathComponent
    return last.isEmpty || last == "." || last == ".." || last == "/" ? fallback : last
}

/// `url`, or `name (2).ext`, `name (3).ext`… beside it if that is taken.
func freeURL(_ url: URL) -> URL {
    let files = FileManager.default
    guard files.fileExists(atPath: url.path) else { return url }
    let folder = url.deletingLastPathComponent()
    let ext = url.pathExtension
    let base = url.deletingPathExtension().lastPathComponent
    var number = 2
    while true {
        let name = ext.isEmpty ? "\(base) (\(number))" : "\(base) (\(number)).\(ext)"
        let candidate = folder.appendingPathComponent(name)
        if !files.fileExists(atPath: candidate.path) {
            return candidate
        }
        number += 1
    }
}

/// The SF Symbol for a device platform, one of `protocol`'s `PLATFORM_*` values.
func platformSymbol(_ platform: String) -> String {
    switch platform {
    case "ios": "iphone"
    case "android": "candybarphone"
    case "desktop-macos": "laptopcomputer"
    case "desktop-linux", "desktop-windows": "desktopcomputer"
    case "web": "globe"
    default: "questionmark.square.dashed"
    }
}

/// A platform in words, for VoiceOver and captions.
func platformName(_ platform: String) -> String {
    switch platform {
    case "ios": "iPhone or iPad"
    case "android": "Android"
    case "desktop-macos": "Mac"
    case "desktop-linux": "Linux"
    case "desktop-windows": "Windows"
    case "web": "Web browser"
    default: platform
    }
}
