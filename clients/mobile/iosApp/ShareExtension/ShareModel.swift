import Foundation
import Observation
import UIKit

/// The extension copies each file into the queue itself, so the queue never asks.
private final class NoPickedSources: PickedFiles {
    func copyFile(source: String, dest: String) throws {
        throw CoreError.Failed(code: "file", detail: "this file can't be read")
    }

    func release(source: String) {}
}

private final class QueueForwarder: AutoUploadListener {
    private let report: @Sendable () -> Void

    init(_ changed: @escaping @MainActor () -> Void) {
        report = { Task { @MainActor in changed() } }
    }

    func changed() {
        report()
    }
}

/// Keeps the extension running for a while after the user leaves the share sheet, as the share
/// sheet can close before the work ends. `onExpiry` runs when iOS is about to stop it.
private final class ExpiringActivity: @unchecked Sendable {
    private let done = DispatchSemaphore(value: 0)

    init(reason: String, onExpiry: @escaping @Sendable () -> Void) {
        ProcessInfo.processInfo.performExpiringActivity(withReason: reason) { [done] expired in
            if expired {
                onExpiry()
                done.signal()
            } else {
                done.wait()
            }
        }
    }

    func end() {
        done.signal()
    }
}

/// What the share sheet is doing.
enum ShareState {
    case loading
    /// The app has no session ready for the extension.
    case signedOut
    case failed(String)
    case ready(CoreClient, SharedContent)
    /// Saving or sending, with how far it is, from 0 to 1.
    case working(String, Double)
}

@MainActor
@Observable
final class ShareModel {
    private(set) var state: ShareState = .loading
    /// A failure of the last save or send, shown where it was started.
    var problem: String?
    /// Tells the extension's view controller whether the sheet may be swiped away.
    @ObservationIgnored var onBusyChanged: ((Bool) -> Void)?

    @ObservationIgnored private let items: [NSExtensionItem]
    @ObservationIgnored private let onFinish: () -> Void
    @ObservationIgnored private let onCancel: () -> Void
    @ObservationIgnored private var content: SharedContent?
    @ObservationIgnored private var client: CoreClient?
    @ObservationIgnored private var saver: ExtensionTokenSaver?
    @ObservationIgnored private var queue: UploadQueue?
    @ObservationIgnored private var uploading: [String: UInt64] = [:]
    @ObservationIgnored private var transfer: String?
    @ObservationIgnored private var cancelled = false

    init(items: [NSExtensionItem], onFinish: @escaping () -> Void, onCancel: @escaping () -> Void) {
        self.items = items
        self.onFinish = onFinish
        self.onCancel = onCancel
    }

    func start() async {
        state = .loading
        do {
            if content == nil {
                content = try await SharedContentReader.read(items)
            }
        } catch {
            state = .failed(error.localizedDescription)
            return
        }
        await signIn()
    }

    private func signIn() async {
        guard let content else { return }
        if let client {
            state = .ready(client, content)
            return
        }
        let store = SessionStore()
        guard let saved = store.loadSession(for: .share) else {
            // The app starts one when it next opens, if someone is signed in.
            store.markNeedsSession(.share)
            state = .signedOut
            return
        }
        let saver = ExtensionTokenSaver(server: saved.server, label: .share, token: saved.refreshToken)
        do {
            let client = try CoreClient.withSession(
                baseUrl: saved.server,
                device: DeviceIdentity(id: store.deviceId, name: UIDevice.current.name),
                listener: saver,
                filesDir: try privateFolder("offline").path
            )
            _ = try await client.restore(refreshToken: saved.refreshToken)
            self.client = client
            self.saver = saver
            state = .ready(client, content)
        } catch CoreError.Unauthorized {
            saver.sessionEnded()
            state = .signedOut
        } catch {
            state = .failed(fileMessage(for: error))
        }
    }

    /// Uploads the shared files into `folder`, Home when `nil`, and closes the sheet once they are
    /// up. Text goes as the file it was written to.
    func save(to folder: FolderCrumb?) async {
        guard let client, let content else { return }
        problem = nil
        let target = folder?.name ?? "Home"
        let what = content.text != nil ? "Saving the text" : content.files.count == 1 ? "Uploading" : "Uploading \(content.files.count) files"
        begin("\(what) to \(target)…")
        let activity = ExpiringActivity(reason: "Uploading to Home Space") { [weak self] in
            Task { @MainActor in self?.queue?.stop() }
        }
        defer { activity.end() }
        do {
            let queue = try openQueue(client: client)
            // A share that ended before its files were up left them here; nobody is waiting for
            // them any more, as the sheet that showed them is gone.
            for item in try queue.items() {
                try? queue.cancel(key: item.key)
            }
            uploading = [:]
            do {
                for file in content.files {
                    let key = try queue.add(
                        parentId: folder?.id, folderName: target, name: file.name, source: nil, mime: file.mime)
                    uploading[key] = file.size
                    // Copied, not moved, so that the user can try again after a failure; off the
                    // main thread, as a video can be large.
                    let source = file.url
                    let dest = URL(fileURLWithPath: queue.copyPath(key: key))
                    try await Task.detached { try FileManager.default.copyItem(at: source, to: dest) }.value
                    try queue.ready(key: key, name: nil)
                }
            } catch {
                // All or nothing: a share that half arrived would be hard to tell from a whole one.
                for key in uploading.keys { try? queue.cancel(key: key) }
                throw SharedContentError.unreadable(content.files.count == 1 ? content.files[0].name : "A file")
            }
            let summary = try await queue.run()
            if cancelled { return }
            let left = try queue.items().filter { uploading[$0.key] != nil }
            if left.isEmpty {
                finish()
                return
            }
            for item in left { try? queue.cancel(key: item.key) }
            fail(
                left.compactMap(\.problem).first
                    ?? (summary.offline ? "Could not reach the server" : "The upload stopped before it finished.")
            )
        } catch CoreError.Unauthorized {
            saver?.sessionEnded()
            self.client = nil
            end()
            state = .signedOut
        } catch {
            fail(error is SharedContentError ? error.localizedDescription : fileMessage(for: error))
        }
    }

    /// Sends the shared files to `target` and closes the sheet once they are sent.
    func send(to target: SendTarget) async {
        guard let client, let content else { return }
        problem = nil
        transfer = nil
        begin("Sending to \(target.recipient)…")
        let listener = ProgressForwarder { [weak self] id, fraction in
            guard let self, !self.cancelled else { return }
            self.transfer = id
            self.state = .working("Sending to \(target.recipient)…", fraction)
        }
        let activity = ExpiringActivity(reason: "Sending from Home Space") { [weak self] in
            // A half-sent transfer can't carry on later, so it is taken back.
            Task { @MainActor in self?.cancelTransfer() }
        }
        defer { activity.end() }
        do {
            try await client.sendFiles(
                files: content.sendFiles, userId: target.userId, deviceId: target.deviceId, listener: listener)
            if !cancelled { finish() }
        } catch CoreError.Unauthorized {
            saver?.sessionEnded()
            self.client = nil
            end()
            state = .signedOut
        } catch {
            if !cancelled { fail(fileMessage(for: error)) }
        }
    }

    func targets() async throws -> [SendTarget] {
        guard let client else { return [] }
        return try await client.sendTargets()
    }

    func folders(in parent: String?) async throws -> [Entry] {
        guard let client else { return [] }
        return try await client.list(parentId: parent).filter { $0.kind == .folder }
    }

    /// The session ended while choosing.
    func sessionEnded() {
        saver?.sessionEnded()
        client = nil
        state = .signedOut
    }

    /// Stops what is going and closes the sheet.
    func cancel() {
        cancelled = true
        if let queue {
            for key in uploading.keys { try? queue.cancel(key: key) }
        }
        cancelTransfer()
        content?.remove()
        onCancel()
    }

    private func cancelTransfer() {
        guard let client, let id = transfer else { return }
        transfer = nil
        Task { try? await client.cancelTransfer(id: id) }
    }

    private func begin(_ title: String) {
        state = .working(title, 0)
        onBusyChanged?(true)
    }

    private func end() {
        onBusyChanged?(false)
    }

    private func fail(_ message: String) {
        end()
        problem = message
        if let client, let content {
            state = .ready(client, content)
        } else {
            state = .failed(message)
        }
    }

    private func finish() {
        end()
        content?.remove()
        onFinish()
    }

    private func openQueue(client: CoreClient) throws -> UploadQueue {
        if let queue { return queue }
        let queue = try UploadQueue.open(
            client: client,
            dataDir: try privateFolder("uploads").path,
            files: NoPickedSources(),
            listener: QueueForwarder { [weak self] in self?.uploadsChanged() }
        )
        self.queue = queue
        return queue
    }

    /// How far the shared files are: those gone from the queue are up.
    private func uploadsChanged() {
        guard case .working(let title, _) = state, let queue, !uploading.isEmpty,
            let items = try? queue.items()
        else { return }
        let total = uploading.values.reduce(0, +)
        guard total > 0 else { return }
        var done: UInt64 = 0
        for (key, size) in uploading {
            done += items.first(where: { $0.key == key }).map { min($0.done, size) } ?? size
        }
        state = .working(title, Double(done) / Double(total))
    }
}
