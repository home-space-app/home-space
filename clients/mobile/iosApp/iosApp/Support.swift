import Foundation
import SwiftUI
import UIKit

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
