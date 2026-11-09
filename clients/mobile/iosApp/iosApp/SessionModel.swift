import Foundation
import Observation
import PhotosUI
import SwiftUI
import UIKit

enum Session {
    /// Trying a saved session; `error` is set when that failed but the session may still work.
    case resuming(server: String, error: String?)
    case signedOut
    case signedIn(client: CoreClient, user: SignedInUser)
}

/// What the server said about how to sign in.
enum Probe {
    case checking
    case ready(SignInOptions)
    case failed(String)
}

/// Forwards download progress to the main actor; reports come from the Rust runtime's threads.
private final class ProgressForwarder: DownloadListener {
    private let report: @Sendable (String, Double) -> Void

    init(model: SessionModel) {
        report = { [weak model] id, fraction in
            Task { @MainActor in
                model?.downloadProgressed(id: id, fraction: fraction)
            }
        }
    }

    func progress(id: String, done: Int64, total: Int64) {
        report(id, total > 0 ? min(Double(done) / Double(total), 1) : 1)
    }
}

/// Forwards live events to the main actor; they come from the Rust runtime's threads.
private final class EventsForwarder: EventsListener {
    private let deliver: @Sendable (LiveEvent?) -> Void

    /// Hands the model each event of the stream numbered `stream`, and `nil` once the session
    /// has ended.
    init(model: SessionModel, stream: Int) {
        deliver = { [weak model] event in
            Task { @MainActor in
                model?.received(event, stream: stream)
            }
        }
    }

    func event(event: LiveEvent) {
        deliver(event)
    }

    func sessionEnded() {
        deliver(nil)
    }
}

/// Changes pushed by the server and gathered for a moment, for each shown folder to tell whether
/// it is out of date.
struct LiveChanges: Equatable {
    var generation = 0
    /// Events may have been missed, as while the app was in the background: reload everything.
    var resync = false
    /// The nodes that changed, and the folders they are in now or were in when deleted; a `nil`
    /// folder is Home.
    var ids = Set<String>()
    var folders = Set<String?>()

    func touches(folder: String?, entries: [Entry]) -> Bool {
        resync || folders.contains(folder) || entries.contains { ids.contains($0.id) }
    }
}

/// A photo or video from the library, as a file of its own. The file the system hands over goes
/// away once this returns.
private struct PickedPhoto: Transferable {
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
private final class BackgroundTime {
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

/// A file ready to hand to the system's export picker; the temporary copy goes when that closes.
struct PendingExport: Identifiable {
    let url: URL
    var id: URL { url }
}

/// The session and the sign-in form. A saved token is only ever spent once, because replaying a
/// rotated refresh token ends the session; that is why resuming lives here and not in a view.
/// A copy with edits not on the server yet, waiting for the user to say whether it may go.
struct RemovalWarning {
    let entry: Entry
    let names: [String]
}

@MainActor
@Observable
final class SessionModel {
    private static let defaultServer = "http://localhost:8080"
    private static let probeDelay = Duration.milliseconds(600)
    /// How long live changes are gathered before the shown folders look at them, so a burst of
    /// them reloads a folder once.
    private static let liveDelay = Duration.milliseconds(300)

    private(set) var session: Session = .signedOut
    private(set) var probe: Probe = .checking
    private(set) var server: String
    private(set) var busy = false
    private(set) var error: String?
    var username = ""
    var password = ""

    /// The files being downloaded. Held here so it outlives the folder views.
    private(set) var downloading = Set<String>()
    /// How far each running download is, from 0 to 1. Apart from `downloading` so that only the
    /// row backgrounds redraw on each report: redrawing a row would rebuild its open menu.
    private(set) var progress: [String: Double] = [:]
    /// The files picked to upload, in the order they go up.
    private(set) var uploads: [QueuedUpload] = []
    /// Changes when the folder on screen is out of date.
    private(set) var reloadKey = 0
    /// What other devices changed lately; each shown folder reloads if it is among them.
    private(set) var liveChanges = LiveChanges()
    /// The file Quick Look is showing.
    var preview: URL?
    /// The copy waiting for the user to say whether its unsent edits may go.
    var removalWarning: RemovalWarning?
    /// The names of the files whose unsent edits signing out would lose, while asking.
    var signOutWarning: [String]?
    /// The saved file waiting for the user to pick where it goes.
    private(set) var pendingExport: PendingExport?
    /// A failure with no dialog of its own to appear in.
    var notice: String?
    /// Runs auto-upload while the app is open and someone is signed in.
    private(set) var foregroundSync: ForegroundSync?

    @ObservationIgnored private let store = SessionStore()
    @ObservationIgnored private var started = false
    @ObservationIgnored private var isInFront = false
    /// The live events socket, open while the app is in front and someone is signed in.
    @ObservationIgnored private var events: EventStream?
    /// Numbers each stream, so that a stopped one's last events are ignored.
    @ObservationIgnored private var eventsGeneration = 0
    @ObservationIgnored private var pendingChanges: LiveChanges?
    @ObservationIgnored private var probeTask: Task<Void, Never>?
    @ObservationIgnored private var probeGeneration = 0
    /// Only the file tapped last opens when its download ends.
    @ObservationIgnored private var lastTapped: String?
    @ObservationIgnored private var uploadTask: Task<Void, Never>?
    @ObservationIgnored private var photoTask: Task<Void, Never>?
    /// Photos not copied yet. They can be reached only in this run of the app, which is also why a
    /// failed one can be retried only while they are here.
    private var pickedPhotos: [String: PhotosPickerItem] = [:]
    @ObservationIgnored private var photosToFetch: [String] = []

    init() {
        server = SessionStore().lastServer ?? Self.defaultServer
        if let saved = store.loadSession() {
            server = saved.server
            session = .resuming(server: saved.server, error: nil)
        }
    }

    /// Begins resuming the saved session or checking the server; only the first call does.
    func start() {
        guard !started else { return }
        started = true
        if let saved = store.loadSession() {
            resume(saved)
        } else {
            probeNow()
        }
    }

    func retryResume() {
        guard let saved = store.loadSession() else {
            backToForm()
            return
        }
        session = .resuming(server: saved.server, error: nil)
        resume(saved)
    }

    private func resume(_ saved: SavedSession) {
        Task {
            do {
                // Finds the client if a background task already restored it.
                if let active = try await ClientHolder.shared.resume() {
                    enter(active)
                } else {
                    backToForm()
                }
            } catch CoreError.Unauthorized {
                backToForm()
            } catch {
                session = .resuming(server: saved.server, error: signInMessage(for: error))
            }
        }
    }

    func serverChanged(to text: String) {
        guard text != server else { return }
        server = text
        error = nil
        startProbe(after: Self.probeDelay)
    }

    /// Asks the server again straight away, such as after a failure.
    func probeNow() {
        startProbe(after: nil)
    }

    private func startProbe(after delay: Duration?) {
        probe = .checking
        probeGeneration += 1
        let generation = probeGeneration
        let typed = server
        probeTask?.cancel()
        probeTask = Task {
            if let delay {
                try? await Task.sleep(for: delay)
            }
            guard generation == probeGeneration else { return }
            let answer: Probe
            do {
                let client = CoreClient(baseUrl: try normalizeServerUrl(input: typed))
                answer = .ready(try await client.providers())
            } catch {
                answer = .failed(signInMessage(for: error))
            }
            // A newer check owns the screen now, and this late answer is for an old address.
            if generation == probeGeneration {
                probe = answer
            }
        }
    }

    func signInWithPassword() {
        let attempt = password
        password = ""
        error = nil
        busy = true
        Task {
            defer { busy = false }
            do {
                let client = try newClient(server: try normalizeServerUrl(input: server))
                let user = try await client.login(
                    username: username.trimmingCharacters(in: .whitespacesAndNewlines),
                    password: attempt
                )
                enter(client, user)
            } catch {
                self.error = signInMessage(for: error)
            }
        }
    }

    /// Signs in through the identity provider. `authenticate` shows the browser sheet and returns
    /// the URL it came back to, or `nil` if the user closed it.
    func signInWithSso(authenticate: (URL) async throws -> URL?) async {
        error = nil
        busy = true
        defer { busy = false }
        do {
            let address = try normalizeServerUrl(input: server)
            let client = try newClient(server: address)
            let request = try client.beginSso()
            guard let url = URL(string: request.url) else {
                throw CoreError.SsoFailed
            }
            guard let callback = try await authenticate(url) else { return }
            let user = try await client.finishSso(
                callbackUrl: callback.absoluteString,
                verifier: request.verifier
            )
            enter(client, user)
        } catch {
            self.error = signInMessage(for: error)
        }
    }

    func reload() {
        reloadKey += 1
    }

    /// Shows a file: at once if the copy here is current, else after downloading it.
    func openFile(client: CoreClient, entry: Entry) {
        lastTapped = entry.id
        guard downloading.insert(entry.id).inserted else { return }
        Task {
            defer { downloadEnded(entry.id) }
            do {
                let path = try await client.openCopy(
                    id: entry.id,
                    listener: ProgressForwarder(model: self)
                )
                // Nothing pops up over another file or after the user has left the app.
                if lastTapped == entry.id, UIApplication.shared.applicationState == .active {
                    preview = URL(fileURLWithPath: path)
                }
                reload()
            } catch {
                downloadFailed(client: client, entry: entry, error: error)
            }
        }
    }

    /// Adds picked files, which the picker copied for us, to the uploads. They go up one at a time,
    /// whichever folder is on screen, and carry on when the app next opens.
    func uploadFiles(files: [URL], parentId: String?, folderName: String) {
        guard let queue = uploadQueue() else { return }
        for file in files {
            do {
                let key = try queue.add(
                    parentId: parentId, folderName: folderName, name: file.lastPathComponent, source: nil)
                do {
                    try FileManager.default.moveItem(at: file, to: URL(fileURLWithPath: queue.copyPath(key: key)))
                    try queue.ready(key: key, name: nil)
                } catch {
                    try? queue.fail(key: key, problem: "This file can't be read. Pick it again.")
                }
            } catch {
                notice = "Could not read “\(file.lastPathComponent)”."
            }
        }
        ContinuedUploads.begin(queue: queue)
        runUploads()
    }

    /// Adds photos and videos from the library to the uploads. Each is copied out of the library
    /// now, since a picked photo can't be reached in the app's next run.
    func uploadPhotos(photos: [PhotosPickerItem], parentId: String?, folderName: String) {
        guard let queue = uploadQueue() else { return }
        for photo in photos {
            guard let key = try? queue.add(parentId: parentId, folderName: folderName, name: "Photo", source: nil)
            else {
                notice = "Could not add the photo."
                continue
            }
            pickedPhotos[key] = photo
            photosToFetch.append(key)
        }
        ContinuedUploads.begin(queue: queue)
        fetchPhotos(queue)
    }

    func retryUpload(upload: QueuedUpload) {
        guard let queue = uploadQueue() else { return }
        try? queue.retry(key: upload.key)
        if !upload.canRetry, pickedPhotos[upload.key] != nil {
            photosToFetch.append(upload.key)
            fetchPhotos(queue)
        }
        ContinuedUploads.begin(queue: queue)
        runUploads()
    }

    /// Everything paused goes again, as one thing the user did.
    func continueUploads() {
        guard let queue = uploadQueue() else { return }
        for upload in uploads where upload.state == .paused {
            try? queue.retry(key: upload.key)
        }
        ContinuedUploads.begin(queue: queue)
        runUploads()
    }

    /// Whether Retry can help: there is a copy, or the photo is still at hand in this run.
    func canRetry(_ upload: QueuedUpload) -> Bool {
        upload.state == .offline || upload.state == .paused || (upload.state == .failed && (upload.canRetry || pickedPhotos[upload.key] != nil))
    }

    /// Gives up on an upload: the row goes now, and a running one stops at its next chunk.
    func cancelUpload(upload: QueuedUpload) {
        pickedPhotos[upload.key] = nil
        photosToFetch.removeAll { $0 == upload.key }
        try? uploadQueue()?.cancel(key: upload.key)
    }

    /// Sends what can go. Starts by itself when the app opens, and when the network comes back.
    func runUploads() {
        guard let queue = uploadQueue() else { return }
        if uploadTask != nil {
            queue.start()
            return
        }
        uploadTask = Task {
            // Some time to finish after the user leaves the app. If that isn't enough, the queue
            // stops and carries on when the app next opens.
            let background = BackgroundTime(name: "Uploads") {
                if !ContinuedUploads.isRunning {
                    queue.stop()
                }
            }
            do {
                _ = try await queue.run()
            } catch CoreError.Unauthorized {
                if !Task.isCancelled {
                    sessionEnded()
                }
            } catch {
                // The rows show it, and the next try is a minute away at most.
            }
            background.end()
            if !Task.isCancelled {
                uploadTask = nil
            }
        }
    }

    private func fetchPhotos(_ queue: UploadQueue) {
        guard photoTask == nil else { return }
        photoTask = Task {
            let background = BackgroundTime(name: "Photos") {}
            while !Task.isCancelled, !photosToFetch.isEmpty {
                let key = photosToFetch.removeFirst()
                guard let photo = pickedPhotos[key] else { continue }
                do {
                    let name = try await fetch(photo, to: URL(fileURLWithPath: queue.copyPath(key: key)))
                    guard !Task.isCancelled else { break }
                    try queue.ready(key: key, name: name)
                    pickedPhotos[key] = nil
                    runUploads()
                } catch {
                    try? queue.fail(key: key, problem: fileMessage(for: error))
                }
            }
            background.end()
            if !Task.isCancelled {
                photoTask = nil
            }
        }
    }

    private func fetch(_ photo: PhotosPickerItem, to copy: URL) async throws -> String {
        guard let picked = try await photo.loadTransferable(type: PickedPhoto.self) else {
            throw CoreError.Failed(code: "photo", detail: "this photo can't be read")
        }
        defer { try? FileManager.default.removeItem(at: picked.url.deletingLastPathComponent()) }
        try? FileManager.default.removeItem(at: copy)
        try FileManager.default.moveItem(at: picked.url, to: copy)
        return picked.url.lastPathComponent
    }

    private func uploadQueue() -> UploadQueue? {
        try? ClientHolder.shared.active?.uploadQueue()
    }

    private func uploadsChanged() {
        guard let queue = uploadQueue(), let items = try? queue.items() else { return }
        let arrived = uploads.contains { old in !items.contains { $0.key == old.key } }
        uploads = items
        ContinuedUploads.report(items)
        if arrived {
            reload()
        }
    }

    private func forgetUploads() {
        uploadTask?.cancel()
        uploadTask = nil
        photoTask?.cancel()
        photoTask = nil
        pickedPhotos = [:]
        photosToFetch = []
        uploads = []
    }

    /// Downloads a file to a temporary place, then asks where to put it: iOS only lets an app
    /// export a file that already exists.
    func saveFile(client: CoreClient, entry: Entry) {
        guard downloading.insert(entry.id).inserted else { return }
        Task {
            defer { downloadEnded(entry.id) }
            let folder = FileManager.default.temporaryDirectory
                .appendingPathComponent("save", isDirectory: true)
                .appendingPathComponent(entry.id, isDirectory: true)
            do {
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                let file = folder.appendingPathComponent(exportName(of: entry))
                try await client.saveCopy(
                    id: entry.id,
                    destPath: file.path,
                    listener: ProgressForwarder(model: self)
                )
                pendingExport = PendingExport(url: file)
            } catch {
                try? FileManager.default.removeItem(at: folder)
                downloadFailed(client: client, entry: entry, error: error)
            }
        }
    }

    /// The export picker closed, however that happened, so its temporary copy can go.
    func exportFinished() {
        guard let export = pendingExport else { return }
        try? FileManager.default.removeItem(at: export.url.deletingLastPathComponent())
        pendingExport = nil
    }

    func cancelDownload(client: CoreClient, entry: Entry) {
        try? client.cancelDownload(id: entry.id)
    }

    /// Keeps a file or folder on this device and in sync.
    func makeAvailableOffline(client: CoreClient, entry: Entry) {
        Task {
            do {
                try await client.makeAvailableOffline(id: entry.id)
            } catch {
                notice = fileMessage(for: error)
            }
            reload()
        }
    }

    /// Deletes this device's copy of a file or folder, after a warning if that loses edits.
    func removeOfflineCopy(client: CoreClient, entry: Entry, confirmed: Bool = false) {
        Task {
            do {
                let unsent = confirmed ? [] : try await client.unsentEdits(id: entry.id)
                if unsent.isEmpty {
                    try await client.removeOfflineCopy(id: entry.id)
                } else {
                    removalWarning = RemovalWarning(entry: entry, names: unsent)
                }
            } catch {
                notice = fileMessage(for: error)
            }
            reload()
        }
    }

    /// Signs out, after a warning if that loses edits.
    func requestSignOut(client: CoreClient) {
        Task {
            let unsent = (try? await client.unsentEdits(id: nil)) ?? []
            if unsent.isEmpty {
                signOut()
            } else {
                signOutWarning = unsent
            }
        }
    }

    /// Quick Look closed, or saved a change to the file.
    func previewEnded() {
        foregroundSync?.checkOfflineCopies()
    }

    fileprivate func downloadProgressed(id: String, fraction: Double) {
        // A report can arrive after the download has ended.
        if downloading.contains(id) {
            progress[id] = fraction
        }
    }

    private func downloadEnded(_ id: String) {
        downloading.remove(id)
        progress[id] = nil
    }

    private func downloadFailed(client: CoreClient, entry: Entry, error: Error) {
        switch error as? CoreError {
        case .Cancelled:
            break
        case .Unauthorized:
            // A late failure from a session that has already been replaced changes nothing.
            if case .signedIn(let current, _) = session, current === client {
                sessionEnded()
            }
        default:
            notice = "Could not download “\(entry.name)”: \(fileMessage(for: error))"
        }
    }

    func signOut() {
        signOutWarning = nil
        let current = session
        let queue = uploadQueue()
        forgetUploads()
        let auto = endSync()
        store.forgetSession()
        forgetDownloads()
        backToForm()
        if case .signedIn(let client, _) = current {
            // Already signed out here; this only revokes the token on the server. The uploads go
            // first, since cancelling them takes the session.
            Task {
                try? await queue?.wipe()
                try? await auto?.wipe()
                try? await client.logout()
            }
        }
    }

    /// The server ended the session while the user was browsing.
    func sessionEnded() {
        let queue = uploadQueue()
        forgetUploads()
        let auto = endSync()
        store.forgetSession()
        forgetDownloads()
        backToForm()
        Task {
            try? await queue?.wipe()
            try? await auto?.wipe()
        }
    }

    /// Called as the app comes to the front and leaves it.
    func appInFront(_ inFront: Bool) {
        isInFront = inFront
        if inFront {
            foregroundSync?.start()
            startEvents()
        } else {
            foregroundSync?.stop()
            stopEvents()
            BackgroundSync.schedule()
        }
    }

    /// Opens the live events socket. Only while the app is in front: iOS suspends the app soon
    /// after it leaves, and the socket would die unnoticed.
    private func startEvents() {
        guard events == nil, isInFront, case .signedIn(let client, _) = session else { return }
        eventsGeneration += 1
        events = client.listenEvents(
            listener: EventsForwarder(model: self, stream: eventsGeneration)
        )
    }

    private func stopEvents() {
        events?.stop()
        events = nil
        eventsGeneration += 1
        pendingChanges = nil
    }

    /// An event of stream `stream`, or `nil` when the session ended.
    fileprivate func received(_ event: LiveEvent?, stream: Int) {
        guard events != nil, stream == eventsGeneration else { return }
        if let event {
            gather(event)
        } else {
            sessionEnded()
        }
    }

    private func gather(_ event: LiveEvent) {
        let first = pendingChanges == nil
        var changes = pendingChanges ?? LiveChanges()
        switch event {
        case .nodeChanged(let id, let parentId):
            changes.ids.insert(id)
            changes.folders.insert(parentId)
        case .resync:
            changes.resync = true
        case .trashChanged, .devicesChanged:
            return
        }
        pendingChanges = changes
        guard first else { return }
        Task { [weak self] in
            try? await Task.sleep(for: Self.liveDelay)
            guard let self, var changes = self.pendingChanges else { return }
            self.pendingChanges = nil
            changes.generation = self.liveChanges.generation + 1
            self.liveChanges = changes
        }
    }

    /// Stops everything that syncs, and returns what to wipe once the session has gone.
    private func endSync() -> AutoUpload? {
        let auto = try? ClientHolder.shared.active?.autoUpload()
        stopEvents()
        foregroundSync?.stop()
        foregroundSync = nil
        ClientHolder.shared.release()
        BackgroundSync.schedule()
        return auto
    }

    private func enter(_ client: CoreClient, _ user: SignedInUser) {
        enter(ClientHolder.shared.adopt(client: client, user: user))
    }

    private func enter(_ active: ActiveSession) {
        username = ""
        error = nil
        session = .signedIn(client: active.client, user: active.user)
        ClientHolder.shared.onUploadsChanged = { [weak self] in self?.uploadsChanged() }
        uploadsChanged()
        let sync = ForegroundSync(
            session: active,
            onSessionEnded: { [weak self] in self?.sessionEnded() },
            onWake: { [weak self] in self?.runUploads() }
        )
        foregroundSync = sync
        if isInFront {
            sync.start()
        }
        startEvents()
    }

    private func forgetDownloads() {
        downloading = []
        progress = [:]
        lastTapped = nil
        preview = nil
    }

    private func backToForm() {
        session = .signedOut
        probeNow()
    }

    private func newClient(server: String) throws -> CoreClient {
        try ClientHolder.shared.newClient(server: server)
    }
}

/// A name that stays inside the temporary folder, whatever the server sent.
private func exportName(of entry: Entry) -> String {
    let name = (entry.name as NSString).lastPathComponent
    return name.isEmpty || name == "." || name == ".." || name == "/" ? entry.id : name
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

/// The sentence to show for a failure, matching the web client's.
private func signInMessage(for error: Error) -> String {
    switch error as? CoreError {
    case .InvalidCredentials:
        "Invalid username or password"
    case .SsoExpired:
        "The sign-in expired. Please try again."
    case .SsoDenied:
        "You are not allowed to sign in with this account."
    case .InvalidServerAddress:
        "The server address must start with http:// or https://"
    case .UnprotectedServerAddress:
        "Use https:// for a server outside your home network"
    case .Unreachable(let detail):
        "Could not reach the server: \(detail)"
    case .Failed(_, let detail):
        detail
    case .Unauthorized:
        "Your session has ended. Please sign in again."
    case .SsoFailed, .InvalidId, .Cancelled, nil:
        "Signing in failed. Please try again."
    }
}
