import BackgroundTasks
import Foundation
import Network
import Photos

/// Runs auto-upload passes and offline copies' sync while the app is in front: on opening it, every
/// minute, when the photo library changes and when the network changes. Stops them when the network
/// no longer allows it.
@MainActor
@Observable
final class ForegroundSync: NSObject {
    private static let interval = Duration.seconds(60)

    /// "Sync only on Wi-Fi" is on, and the phone isn't on it.
    private(set) var waitingForWifi = false

    @ObservationIgnored private let session: ActiveSession
    @ObservationIgnored private let onSessionEnded: () -> Void
    /// Each time a pass is due, and when the network changes, whatever "Sync only on Wi-Fi" says.
    @ObservationIgnored private let onWake: () -> Void
    @ObservationIgnored private var timer: Task<Void, Never>?
    @ObservationIgnored private var monitor: NWPathMonitor?
    @ObservationIgnored private var path: NWPath?
    @ObservationIgnored private var observingPhotos = false

    init(session: ActiveSession, onSessionEnded: @escaping () -> Void, onWake: @escaping () -> Void) {
        self.session = session
        self.onSessionEnded = onSessionEnded
        self.onWake = onWake
    }

    func start() {
        guard timer == nil else { return }
        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            Task { @MainActor in self?.networkChanged(path) }
        }
        monitor.start(queue: .global(qos: .utility))
        self.monitor = monitor
        timer = Task { [weak self] in
            while !Task.isCancelled {
                await self?.pass(retryNow: false)
                try? await Task.sleep(for: Self.interval)
            }
        }
        observePhotos()
    }

    /// Leaves a pass running only if syncing in the background is allowed.
    func stop() {
        timer?.cancel()
        timer = nil
        monitor?.cancel()
        monitor = nil
        if let auto = try? session.autoUpload(), (try? auto.settings().background) != true {
            auto.stop()
        }
        // Nothing watches the network from here on. Background tasks check it for themselves.
        allowOffline(false)
    }

    /// "Sync now", or a change of settings: tries at once what was waiting to be tried again.
    func syncNow() {
        Task { await pass(retryNow: true) }
    }

    /// Looks at the offline copies now, such as after a viewer closed. Retries nothing.
    func checkOfflineCopies() {
        guard networkAllows() else { return }
        let client = session.client
        Task { try? await client.offlineSyncNow(retryFailed: false) }
    }

    private func allowOffline(_ allowed: Bool) {
        let client = session.client
        Task { try? await client.offlineAllow(allowed: allowed) }
    }

    /// The library can be looked at only after the user has allowed it, so `start` isn't enough.
    func observePhotos() {
        let status = PHPhotoLibrary.authorizationStatus(for: .readWrite)
        guard !observingPhotos, status == .authorized || status == .limited else { return }
        observingPhotos = true
        PHPhotoLibrary.shared().register(self)
    }

    private func networkChanged(_ path: NWPath) {
        self.path = path
        onWake()
        if applyNetworkRule() {
            Task { await pass(retryNow: false) }
        }
    }

    /// Whether the network may be used, stopping what is under way if not. Also after a change of
    /// "Sync only on Wi-Fi", which no network callback reports.
    private func applyNetworkRule() -> Bool {
        let allowed = networkAllows()
        waitingForWifi = !allowed
        if !allowed {
            (try? session.autoUpload())?.stop()
            allowOffline(false)
        }
        return allowed
    }

    private func networkAllows() -> Bool {
        guard let path else { return false }
        return networkAllowsUse(of: path, session: session)
    }

    private func pass(retryNow: Bool) async {
        onWake()
        // Until the monitor first reports, nothing is known, and a background pass may be running.
        guard path != nil, applyNetworkRule(), let auto = try? session.autoUpload() else { return }
        do {
            try? await session.client.offlineAllow(allowed: true)
            if retryNow {
                try? await session.client.offlineSyncNow(retryFailed: true)
            }
            _ = try await auto.runPass(retryNow: retryNow)
        } catch CoreError.Unauthorized {
            onSessionEnded()
        } catch {
            // Shown per source on the settings screen; the next pass tries again.
        }
    }
}

extension ForegroundSync: PHPhotoLibraryChangeObserver {
    nonisolated func photoLibraryDidChange(_ changeInstance: PHChange) {
        Task { @MainActor in await self.pass(retryNow: false) }
    }
}

@MainActor
private func networkAllowsUse(of path: NWPath, session: ActiveSession) -> Bool {
    guard path.status == .satisfied else { return false }
    let wifiOnly = (try? session.autoUpload().settings().wifiOnly) ?? true
    return !wifiOnly || !path.isExpensive
}

/// Runs passes while the app isn't open, when "Sync in background" is on. iOS decides when: a
/// refresh is short and comes now and then, and processing tends to wait for the phone to charge.
enum BackgroundSync {
    // Also listed in `project.yml`, as BGTaskSchedulerPermittedIdentifiers.
    private static let refresh = "app.homespace.ios.sync-refresh"
    private static let processing = "app.homespace.ios.sync-processing"

    /// Must be called before the app finishes launching.
    static func register() {
        for identifier in [refresh, processing] {
            BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: nil) { task in
                Task { @MainActor in await run(task) }
            }
        }
    }

    /// Asks iOS for the next run, or takes back the request if the setting is off.
    @MainActor
    static func schedule() {
        guard let auto = try? ClientHolder.shared.active?.autoUpload(),
            (try? auto.settings().background) == true
        else {
            // Only these two: an upload the user started goes on, whatever this setting says.
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: refresh)
            BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: processing)
            return
        }
        let quick = BGAppRefreshTaskRequest(identifier: refresh)
        try? BGTaskScheduler.shared.submit(quick)
        let long = BGProcessingTaskRequest(identifier: processing)
        long.requiresNetworkConnectivity = true
        try? BGTaskScheduler.shared.submit(long)
    }

    @MainActor
    private static func run(_ task: BGTask) async {
        // Keeps running after the next is asked for, since only one of each can be waiting.
        defer { schedule() }
        guard let session = try? await ClientHolder.shared.resume(),
            let auto = try? session.autoUpload(),
            (try? auto.settings().background) == true,
            await isNetworkAllowed(session)
        else {
            task.setTaskCompleted(success: true)
            return
        }
        let client = session.client
        task.expirationHandler = {
            auto.stop()
            Task { try? await client.offlineAllow(allowed: false) }
        }
        let summary = try? await auto.runPass(retryNow: false)
        try? await client.offlinePass()
        // There is no push, so this is how transfers sent while the app was away get noticed.
        if let lists = try? await client.transfers() {
            await IncomingNotices.update(lists.received, quiet: false)
        }
        // Nothing watches the network once this task ends. An open app allows it again.
        try? await client.offlineAllow(allowed: false)
        task.setTaskCompleted(success: summary != nil)
    }

    @MainActor
    private static func isNetworkAllowed(_ session: ActiveSession) async -> Bool {
        let path = await withCheckedContinuation { (continuation: CheckedContinuation<NWPath, Never>) in
            let monitor = NWPathMonitor()
            monitor.pathUpdateHandler = { path in
                monitor.cancel()
                continuation.resume(returning: path)
            }
            monitor.start(queue: .global(qos: .utility))
        }
        return networkAllowsUse(of: path, session: session)
    }
}

/// Lets the picked files carry on after the user leaves the app, where iOS 26 offers it: the system
/// shows the progress and may end the task, and the user can end it from there. It must start from
/// something the user did, so it starts on picking files, Retry or Continue, not when the app opens. On
/// earlier versions, and when it can't start, uploads get only the short time any app gets.
enum ContinuedUploads {
    /// Whether the task is keeping the app running, so its short background time doesn't matter.
    @MainActor
    static var isRunning: Bool {
        if #available(iOS 26.0, *) {
            return ContinuedUploadTask.shared.isRunning
        }
        return false
    }

    /// Call it when the user has just asked for an upload.
    @MainActor
    static func begin(queue: UploadQueue) {
        if #available(iOS 26.0, *) {
            ContinuedUploadTask.shared.begin(queue: queue)
        }
    }

    /// Call it when the queue has changed, to update what the system shows.
    @MainActor
    static func report(_ items: [QueuedUpload]) {
        if #available(iOS 26.0, *) {
            ContinuedUploadTask.shared.report(items)
        }
    }
}

@available(iOS 26.0, *)
@MainActor
private final class ContinuedUploadTask {
    static let shared = ContinuedUploadTask()

    // Under the wildcard listed in `project.yml`, as BGTaskSchedulerPermittedIdentifiers.
    private static let identifier = "app.homespace.ios.uploads.picked"
    private static let unitsPerFile: Int64 = 1000

    private(set) var isRunning = false
    private var isRegistered = false
    private var task: BGContinuedProcessingTask?
    private var queue: UploadQueue?
    /// The files this task has seen, and how many of them have gone.
    private var known = Set<String>()
    private var finished = 0

    func begin(queue: UploadQueue) {
        guard !isRunning else { return }
        register()
        self.queue = queue
        known = []
        finished = 0
        let request = BGContinuedProcessingTaskRequest(
            identifier: Self.identifier,
            title: "Uploading files",
            subtitle: "Home Space"
        )
        // If it can't run now, the upload goes on while the app is open, with nothing left pending.
        request.strategy = .fail
        do {
            try BGTaskScheduler.shared.submit(request)
            isRunning = true
        } catch {
            self.queue = nil
        }
    }

    /// Only once in a run, or the system ends the app. It may come after launch for this kind of task.
    private func register() {
        guard !isRegistered else { return }
        isRegistered = true
        BGTaskScheduler.shared.register(forTaskWithIdentifier: Self.identifier, using: nil) { task in
            guard let task = task as? BGContinuedProcessingTask else {
                task.setTaskCompleted(success: false)
                return
            }
            Task { @MainActor in await ContinuedUploadTask.shared.run(task) }
        }
    }

    private func run(_ task: BGContinuedProcessingTask) async {
        guard let queue else {
            task.setTaskCompleted(success: false)
            isRunning = false
            return
        }
        self.task = task
        var expired = false
        // iOS doesn't say whether it or the user ended the task, so the files wait for the user
        // either way. Continuing is something the user did, so it can start a task again.
        task.expirationHandler = {
            try? queue.pause()
            Task { @MainActor in expired = true }
        }
        report((try? queue.items()) ?? [])
        var succeeded = false
        while !expired {
            let summary = try? await queue.run()
            succeeded = summary?.remaining == 0 && summary?.failed == 0
            // A photo still being fetched goes up once it is here.
            let fetching = (try? queue.items())?.contains { $0.state == .preparing } ?? false
            guard !expired, fetching, summary?.stopped == false, summary?.offline == false else { break }
            try? await Task.sleep(for: .seconds(1))
        }
        // `report` has nothing to show once the queue is empty.
        if succeeded && !expired {
            task.progress.completedUnitCount = task.progress.totalUnitCount
        }
        self.task = nil
        self.queue = nil
        isRunning = false
        task.setTaskCompleted(success: succeeded && !expired)
    }

    /// Each file counts the same, so that the bar doesn't jump when a copy's size becomes known.
    func report(_ items: [QueuedUpload]) {
        guard let task else { return }
        let going = items.filter { $0.state != .failed && $0.state != .paused }
        guard !going.isEmpty else { return }
        finished = max(finished, known.subtracting(going.map(\.key)).count)
        known.formUnion(going.map(\.key))
        let part = going.first { $0.state == .uploading }
        let fraction = part.map { $0.total > 0 ? Double($0.done) / Double($0.total) : 0 } ?? 0
        task.progress.totalUnitCount = Int64(known.count) * Self.unitsPerFile
        task.progress.completedUnitCount =
            Int64(finished) * Self.unitsPerFile + Int64(fraction * Double(Self.unitsPerFile))
        let title = going.count == 1 ? "Uploading \(going[0].name)" : "Uploading \(going.count) files"
        task.updateTitle(title, subtitle: "To \(going[0].folderName)")
    }
}
