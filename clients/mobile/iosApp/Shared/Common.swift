import Foundation

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

/// The sentence to show when a file operation fails.
func fileMessage(for error: Error) -> String {
    switch error as? CoreError {
    case .Failed(_, let detail):
        detail.prefix(1).uppercased() + detail.dropFirst()
    case .Unreachable:
        "Could not reach the server"
    default:
        error.localizedDescription
    }
}
