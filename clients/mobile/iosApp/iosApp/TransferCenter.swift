import BackgroundTasks
import Foundation
import Observation
import UIKit
import UniformTypeIdentifiers

/// A received link waiting for the user to say it may open.
struct PendingLink: Identifiable {
    let transferId: String
    let link: TransferLink
    var id: String { transferId }
}

/// A received transfer that is now in Files, to tell the user where.
struct SavedTransfer: Identifiable {
    let title: String
    /// What was saved, in the app's folder of Files.
    let url: URL
    var id: URL { url }
}

/// What is about to be sent to a device.
enum SendPayload: Identifiable {
    /// Files and folders of the user's own, which the server copies.
    case entries([Entry])
    /// Files on this phone, copied into `folder`, which goes once they are sent.
    case files([SendFile], folder: URL)

    var id: String {
        switch self {
        case .entries(let entries): "entries-" + entries.map(\.id).joined(separator: ",")
        case .files(_, let folder): "files-" + folder.lastPathComponent
        }
    }

    var count: Int {
        switch self {
        case .entries(let entries): entries.count
        case .files(let files, _): files.count
        }
    }
}

/// Send to device: what was sent to this user and what they sent, and the downloads and sends
/// running now. Lives as long as the session, so that progress outlives the Transfers tab.
@MainActor
@Observable
final class TransferCenter {
    /// How often the lists reload while the app is open, so that time left stays right.
    private static let refreshInterval = Duration.seconds(60)

    private(set) var received: [TransferRow] = []
    private(set) var sent: [TransferRow] = []
    /// Whether the lists have loaded once.
    private(set) var loaded = false
    private(set) var error: String?
    /// The received transfers being downloaded to this phone.
    private(set) var downloading = Set<String>()
    /// How far each download or send is, by transfer id, from 0 to 1.
    private(set) var progress: [String: Double] = [:]
    /// Sends started here whose transfer the server hasn't listed yet, to show them at once.
    private(set) var preparing = 0
    var pendingLink: PendingLink?
    var saved: SavedTransfer?
    var notice: String?

    let client: CoreClient
    @ObservationIgnored private let onSessionEnded: () -> Void
    @ObservationIgnored private var timer: Task<Void, Never>?
    @ObservationIgnored private var generation = 0
    /// The sends running, by the id the listener reported, so iOS ending the app can stop them.
    @ObservationIgnored private var sends: [String: Bool] = [:]

    init(client: CoreClient, onSessionEnded: @escaping () -> Void) {
        self.client = client
        self.onSessionEnded = onSessionEnded
    }

    /// Received transfers waiting for this user to act, for the tab's badge.
    var waiting: Int {
        received.count { $0.status == .ready && !downloading.contains($0.id) }
    }

    var isSending: Bool { preparing > 0 || !sends.isEmpty }

    // MARK: Loading

    /// Reloads now and then while the app is in front.
    func start() {
        guard timer == nil else { return }
        timer = Task { [weak self] in
            while !Task.isCancelled {
                await self?.reload()
                try? await Task.sleep(for: Self.refreshInterval)
            }
        }
    }

    func stop() {
        timer?.cancel()
        timer = nil
    }

    func refresh() {
        Task { await reload() }
    }

    func reload() async {
        generation += 1
        let mine = generation
        do {
            let lists = try await client.transfers()
            // An older answer can't replace a newer one.
            guard mine == generation else { return }
            received = lists.received
            sent = lists.sent
            loaded = true
            error = nil
            await IncomingNotices.update(lists.received, quiet: AppRouter.shared.transfersVisible)
        } catch {
            guard mine == generation else { return }
            failed(error, shown: false)
        }
    }

    /// Stops everything, for sign-out.
    func end() {
        stop()
        for id in downloading {
            try? client.stopTransferDownload(id: id)
        }
        for id in sends.keys {
            try? client.stopTransferDownload(id: id)
        }
    }

    // MARK: Received

    /// Asks before opening a received link.
    func open(_ row: TransferRow) {
        Task {
            do {
                pendingLink = PendingLink(transferId: row.id, link: try await client.transferLink(id: row.id))
            } catch {
                failed(error)
            }
        }
    }

    /// Opens a link the user confirmed, then tells the server it arrived.
    func openConfirmed(_ pending: PendingLink) {
        pendingLink = nil
        guard let url = URL(string: pending.link.uri) else {
            notice = "This link can't be opened."
            return
        }
        Task {
            guard await UIApplication.shared.open(url) else {
                notice = "No app on this iPhone opens \(pending.link.scheme) links."
                return
            }
            do {
                try await client.confirmTransfer(id: pending.transferId)
            } catch {
                failed(error)
            }
            await reload()
        }
    }

    /// Moves a received transfer into the user's files on the server.
    func save(_ row: TransferRow, to parentId: String?) {
        Task {
            do {
                try await client.saveTransfer(id: row.id, parentId: parentId)
            } catch {
                failed(error)
            }
            await reload()
        }
    }

    func decline(_ row: TransferRow) {
        Task {
            do {
                try await client.declineTransfer(id: row.id)
            } catch {
                failed(error)
            }
            await reload()
        }
    }

    /// Takes back a sent transfer, stopping its upload if it is still running here.
    func cancel(_ row: TransferRow) {
        Task {
            do {
                try await client.cancelTransfer(id: row.id)
            } catch {
                failed(error)
            }
            await reload()
        }
    }

    func stopDownload(_ row: TransferRow) {
        try? client.stopTransferDownload(id: row.id)
    }

    /// Downloads a received transfer into the app's folder in Files, keeping its folders, then
    /// tells the server it arrived.
    func download(_ row: TransferRow) {
        let id = row.id
        guard downloading.insert(id).inserted else { return }
        let client = client
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("transfers", isDirectory: true)
            .appendingPathComponent(id, isDirectory: true)
        // Some time to finish after the user leaves the app; the transfer stays on the server if
        // that isn't enough.
        let background = BackgroundTime(name: "Transfer") {
            try? client.stopTransferDownload(id: id)
        }
        Task {
            defer {
                background.end()
                downloading.remove(id)
                progress[id] = nil
                try? FileManager.default.removeItem(at: folder)
            }
            do {
                _ = try await client.downloadTransfer(id: id, destDir: folder.path, listener: listener())
                let kept = try Self.keep(contentsOf: folder)
                try await client.confirmTransfer(id: id)
                if let kept {
                    saved = SavedTransfer(title: row.title, url: kept)
                }
            } catch {
                failed(error)
            }
            await reload()
        }
    }

    /// The folder that received files go to: On My iPhone › Home Space › Received in Files.
    static func receivedFolder() throws -> URL {
        let documents = try FileManager.default.url(
            for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        let folder = documents.appendingPathComponent("Received", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder
    }

    /// Moves what was downloaded into `receivedFolder`, giving taken names a number. Returns the
    /// file or folder to show, or the folder itself when there were several.
    private static func keep(contentsOf downloaded: URL) throws -> URL? {
        let files = FileManager.default
        let destination = try receivedFolder()
        let items = try files.contentsOfDirectory(at: downloaded, includingPropertiesForKeys: nil)
        var moved: [URL] = []
        for item in items {
            let target = freeURL(destination.appendingPathComponent(item.lastPathComponent))
            try files.moveItem(at: item, to: target)
            moved.append(target)
        }
        return moved.count == 1 ? moved[0] : (moved.isEmpty ? nil : destination)
    }

    // MARK: Sending

    func targets() async throws -> [SendTarget] {
        try await client.sendTargets()
    }

    /// Sends files and folders of the user's own; the server makes the copy, so this is quick.
    func send(entries: [Entry], to target: SendTarget) async throws {
        try await client.sendEntries(entries: entries, userId: target.userId, deviceId: target.deviceId)
        await reload()
    }

    /// Sends files from this phone, with progress in the Sent list. Runs on after the sheet
    /// closes, and for a while after the user leaves the app.
    func send(files: [SendFile], folder: URL, to target: SendTarget) {
        preparing += 1
        var transfer: String?
        let client = client
        let listener = ProgressForwarder { [weak self] id, fraction in
            guard let self else { return }
            if transfer == nil {
                // The first report: the transfer exists on the server now.
                transfer = id
                self.preparing -= 1
                self.sends[id] = true
                self.refresh()
            }
            self.progress[id] = fraction
            ContinuedSends.report(self.sendFraction)
        }
        let background = BackgroundTime(name: "Send") {
            // A half-sent transfer can't carry on later, so it is taken back.
            guard !ContinuedSends.isRunning, let id = transfer else { return }
            Task { try? await client.cancelTransfer(id: id) }
            SendNotices.stopped(recipient: target.recipient)
        }
        ContinuedSends.begin(
            title: files.count == 1 ? "Sending \(files[0].name)" : "Sending \(files.count) files",
            subtitle: "To \(target.recipient)",
            center: self
        )
        Task {
            defer {
                background.end()
                if let id = transfer {
                    sends[id] = nil
                    progress[id] = nil
                } else {
                    preparing -= 1
                }
                try? FileManager.default.removeItem(at: folder)
                ContinuedSends.finished(stillSending: isSending)
            }
            do {
                try await client.sendFiles(
                    files: files, userId: target.userId, deviceId: target.deviceId, listener: listener)
            } catch {
                failed(error)
            }
            await reload()
            await IncomingNotices.askIfNeeded()
        }
    }

    /// Takes back every send running here, as when iOS ends the time the app was given.
    func cancelSends() {
        for id in sends.keys {
            Task { try? await client.cancelTransfer(id: id) }
        }
    }

    /// How far the running sends are, together.
    var sendFraction: Double {
        let running = sends.keys.compactMap { progress[$0] }
        guard !running.isEmpty else { return 0 }
        return running.reduce(0, +) / Double(running.count + preparing)
    }

    /// Copies picked files, which the picker already copied for us, into a folder of their own.
    static func prepare(files picked: [URL]) throws -> SendPayload {
        let folder = try sendFolder()
        var files: [SendFile] = []
        for url in picked {
            let target = freeURL(folder.appendingPathComponent(url.lastPathComponent))
            try FileManager.default.moveItem(at: url, to: target)
            files.append(SendFile(name: target.lastPathComponent, path: target.path, mime: mime(of: target)))
        }
        return .files(files, folder: folder)
    }

    /// A new folder for the files of one send.
    static func sendFolder() throws -> URL {
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("send", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder
    }

    static func mime(of url: URL) -> String? {
        UTType(filenameExtension: url.pathExtension)?.preferredMIMEType
    }

    // MARK: Helpers

    private func listener() -> ProgressForwarder {
        ProgressForwarder { [weak self] id, fraction in
            guard let self, self.downloading.contains(id) else { return }
            self.progress[id] = fraction
        }
    }

    private func failed(_ error: Error, shown: Bool = true) {
        switch error as? CoreError {
        case .Cancelled:
            break
        case .Unauthorized:
            onSessionEnded()
        default:
            if shown {
                notice = fileMessage(for: error)
            } else {
                self.error = fileMessage(for: error)
            }
        }
    }
}

/// Lets a send from this phone carry on after the user leaves the app, where iOS 26 offers it.
/// Like `ContinuedUploads`, it must start from something the user did.
@MainActor
enum ContinuedSends {
    static var isRunning: Bool {
        if #available(iOS 26.0, *) {
            return ContinuedSendTask.shared.isRunning
        }
        return false
    }

    static func begin(title: String, subtitle: String, center: TransferCenter) {
        if #available(iOS 26.0, *) {
            ContinuedSendTask.shared.begin(title: title, subtitle: subtitle, center: center)
        }
    }

    static func report(_ fraction: Double) {
        if #available(iOS 26.0, *) {
            ContinuedSendTask.shared.report(fraction)
        }
    }

    static func finished(stillSending: Bool) {
        if #available(iOS 26.0, *), !stillSending {
            ContinuedSendTask.shared.finish()
        }
    }
}

@available(iOS 26.0, *)
@MainActor
private final class ContinuedSendTask {
    static let shared = ContinuedSendTask()

    // Under the wildcard listed in `project.yml`, as BGTaskSchedulerPermittedIdentifiers.
    private static let identifier = "app.homespace.ios.uploads.send"
    private static let units: Int64 = 1000

    private(set) var isRunning = false
    private var isRegistered = false
    private var task: BGContinuedProcessingTask?
    private weak var center: TransferCenter?
    private var done: CheckedContinuation<Void, Never>?

    func begin(title: String, subtitle: String, center: TransferCenter) {
        guard !isRunning else { return }
        register()
        self.center = center
        let request = BGContinuedProcessingTaskRequest(
            identifier: Self.identifier, title: title, subtitle: subtitle)
        request.strategy = .fail
        do {
            try BGTaskScheduler.shared.submit(request)
            isRunning = true
        } catch {
            self.center = nil
        }
    }

    private func register() {
        guard !isRegistered else { return }
        isRegistered = true
        BGTaskScheduler.shared.register(forTaskWithIdentifier: Self.identifier, using: nil) { task in
            guard let task = task as? BGContinuedProcessingTask else {
                task.setTaskCompleted(success: false)
                return
            }
            Task { @MainActor in await ContinuedSendTask.shared.run(task) }
        }
    }

    private func run(_ task: BGContinuedProcessingTask) async {
        guard let center, center.isSending else {
            task.setTaskCompleted(success: center != nil)
            isRunning = false
            return
        }
        self.task = task
        task.progress.totalUnitCount = Self.units
        var expired = false
        task.expirationHandler = {
            Task { @MainActor in
                expired = true
                center.cancelSends()
                ContinuedSendTask.shared.finish()
            }
        }
        await withCheckedContinuation { done = $0 }
        self.task = nil
        self.center = nil
        isRunning = false
        task.setTaskCompleted(success: !expired)
    }

    func report(_ fraction: Double) {
        task?.progress.completedUnitCount = Int64(fraction * Double(Self.units))
    }

    func finish() {
        if let task {
            task.progress.completedUnitCount = task.progress.totalUnitCount
        }
        done?.resume()
        done = nil
        if task == nil {
            isRunning = false
            center = nil
        }
    }
}
