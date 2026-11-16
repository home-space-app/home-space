import SwiftUI

/// Everything on its way to or from this phone: what other devices sent, picked files, what
/// syncs by itself, then what this user sent.
struct TransfersView: View {
    let model: SessionModel
    let client: CoreClient
    @State private var sources: [UploadSource] = []
    @State private var current: CurrentUpload?
    @State private var offline: OfflineStatus?
    /// The received transfer whose choices are showing.
    @State private var acting: TransferRow?
    /// The received transfer waiting for a folder to be saved into.
    @State private var saving: TransferRow?
    @State private var declining: TransferRow?

    private var kept: UInt64 { offline?.kept ?? 0 }
    private var center: TransferCenter? { model.transfers }
    private var received: [TransferRow] { center?.received ?? [] }
    private var sent: [TransferRow] { center?.sent ?? [] }

    var body: some View {
        List {
            if let error = center?.error {
                Text(error).foregroundStyle(.red)
            }
            if !received.isEmpty {
                Section("Received") {
                    ForEach(received, id: \.id) { row in
                        receivedRow(row)
                    }
                }
            }
            if !model.uploads.isEmpty {
                Section {
                    ForEach(model.uploads, id: \.key) { upload in
                        UploadRow(model: model, upload: upload)
                    }
                } header: {
                    HStack {
                        Text("Uploads")
                        Spacer()
                        if model.uploads.count(where: { $0.state == .paused }) > 1 {
                            Button("Continue All") { model.continueUploads() }
                                .font(.subheadline)
                                .textCase(nil)
                        }
                    }
                }
            }
            if !sources.isEmpty {
                Section("Auto-Upload") {
                    ForEach(sources) { source in
                        NavigationLink {
                            SyncSettingsView(model: model, client: client)
                        } label: {
                            sourceLabel(source, current: current?.sourceId == source.id ? current : nil)
                        }
                    }
                }
            }
            if let offline, offline.kept > 0 {
                Section("Offline Copies") {
                    NavigationLink {
                        SyncSettingsView(model: model, client: client)
                    } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(offline.kept == 1 ? "1 item on this iPhone" : "\(offline.kept) items on this iPhone")
                            Text(detail(of: offline))
                                .font(.caption)
                                .foregroundStyle(offline.problems.isEmpty ? Color.secondary : Color.red)
                        }
                    }
                }
            }
            if !sent.isEmpty || (center?.preparing ?? 0) > 0 {
                Section("Sent") {
                    if let preparing = center?.preparing, preparing > 0 {
                        HStack(spacing: 12) {
                            ProgressView().frame(width: 26, height: 26)
                            Text(preparing == 1 ? "Starting to send…" : "Starting \(preparing) sends…")
                                .foregroundStyle(.secondary)
                        }
                    }
                    ForEach(sent, id: \.id) { row in
                        sentRow(row)
                    }
                }
            }
        }
        .overlay {
            if model.uploads.isEmpty && sources.isEmpty && kept == 0 && received.isEmpty && sent.isEmpty
                && (center?.preparing ?? 0) == 0 && center?.loaded == true
            {
                ContentUnavailableView(
                    "No Transfers",
                    systemImage: "arrow.up.arrow.down",
                    description: Text("Files sent to this iPhone, files you upload and folders that sync show up here.")
                )
            }
        }
        .navigationTitle("Transfers")
        .refreshable {
            await center?.reload()
            await load()
        }
        .task(id: ClientHolder.shared.changes.generation) { await load() }
        .task { await IncomingNotices.askIfNeeded() }
        // A notification's Open, once the list has the transfer.
        .task(id: OpenRequest(id: AppRouter.shared.openTransfer, loaded: center?.loaded ?? false)) {
            guard let id = AppRouter.shared.openTransfer, let center, center.loaded else { return }
            AppRouter.shared.openTransfer = nil
            if let row = center.received.first(where: { $0.id == id }), row.status == .ready, row.canOpen {
                center.open(row)
            }
        }
        .confirmationDialog(
            acting?.title ?? "",
            isPresented: Binding(get: { acting != nil }, set: { if !$0 { acting = nil } }),
            titleVisibility: .visible,
            presenting: acting
        ) { row in
            if row.canOpen {
                Button("Open") { center?.open(row) }
            }
            Button("Save to My Files…") { saving = row }
            Button("Download to iPhone") { center?.download(row) }
            Button("Decline", role: .destructive) { declining = row }
            Button("Cancel", role: .cancel) {}
        } message: { row in
            Text([row.party, row.sizeLabel, row.note].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
        }
        .confirmationDialog(
            "Decline “\(declining?.title ?? "")”?",
            isPresented: Binding(get: { declining != nil }, set: { if !$0 { declining = nil } }),
            titleVisibility: .visible,
            presenting: declining
        ) { row in
            Button("Decline", role: .destructive) { center?.decline(row) }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("The sender sees that you declined it, and the files are deleted from the server.")
        }
        .sheet(item: $saving) { row in
            ServerFolderSheet(
                title: "Save to My Files",
                confirm: "Save",
                askUploadExisting: false,
                model: model,
                client: client
            ) { parent, _, _ in
                center?.save(row, to: parent)
            }
        }
    }

    @ViewBuilder
    private func receivedRow(_ row: TransferRow) -> some View {
        let downloading = center?.downloading.contains(row.id) == true
        let actionable = row.status == .ready && !downloading
        Button {
            if actionable {
                acting = row
            }
        } label: {
            TransferRowLabel(row: row, progress: center?.progress[row.id], received: true, busy: downloading)
        }
        .buttonStyle(.plain)
        .disabled(!actionable && !downloading)
        .swipeActions {
            if downloading {
                Button("Stop") { center?.stopDownload(row) }
            } else if row.status == .ready {
                Button("Decline", role: .destructive) { declining = row }
            }
        }
        .contextMenu {
            if downloading {
                Button("Stop Download", systemImage: "stop.circle") { center?.stopDownload(row) }
            } else if row.status == .ready {
                if row.canOpen {
                    Button("Open", systemImage: "arrow.up.forward.app") { center?.open(row) }
                }
                Button("Save to My Files…", systemImage: "folder.badge.plus") { saving = row }
                Button("Download to iPhone", systemImage: "arrow.down.circle") { center?.download(row) }
                Button("Decline", systemImage: "xmark.circle", role: .destructive) { declining = row }
            }
        }
    }

    @ViewBuilder
    private func sentRow(_ row: TransferRow) -> some View {
        let cancellable = row.status == .uploading || row.status == .ready
        TransferRowLabel(row: row, progress: center?.progress[row.id], received: false, busy: false)
            .swipeActions {
                if cancellable {
                    Button("Cancel", role: .destructive) { center?.cancel(row) }
                }
            }
            .contextMenu {
                if cancellable {
                    Button("Cancel Transfer", systemImage: "xmark.circle", role: .destructive) { center?.cancel(row) }
                }
            }
    }

    private func load() async {
        let auto = try? ClientHolder.shared.active?.autoUpload()
        sources = (try? auto?.sources()) ?? []
        current = auto?.current()
        offline = try? await client.offlineStatus()
    }

    private func sourceLabel(_ source: UploadSource, current: CurrentUpload?) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(source.label).lineLimit(1)
            if let current {
                Text("Uploading \(current.name)\(percent(done: current.done, total: current.total))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                Text(status(of: source))
                    .font(.caption)
                    .foregroundStyle(source.problem != nil || source.lastError != nil ? Color.red : Color.secondary)
            }
        }
    }

    private func status(of source: UploadSource) -> String {
        if let problem = source.problem {
            return problem
        }
        if let failure = source.lastError {
            return "\(source.pending) to go. \(failure)"
        }
        if source.pending > 0 {
            return "\(source.pending) to go"
        }
        return source.lastSyncedAt == nil ? "Waiting to sync" : "Up to date"
    }

    private func detail(of status: OfflineStatus) -> String {
        if !status.problems.isEmpty {
            return "\(status.problems.count) couldn’t sync"
        }
        if status.unreachable {
            return "Waiting for the connection"
        }
        return status.pending > 0 ? "\(status.pending) updating" : "Up to date"
    }
}

extension TransferRow: Identifiable {}

/// What makes a notification's Open look for its transfer again.
private struct OpenRequest: Equatable {
    let id: String?
    let loaded: Bool
}

/// A transfer in the Received or Sent list.
private struct TransferRowLabel: View {
    let row: TransferRow
    let progress: Double?
    let received: Bool
    let busy: Bool

    private var symbol: String {
        if row.canOpen {
            return "link"
        }
        return received ? "arrow.down.doc" : "paperplane"
    }

    private var status: String {
        if busy {
            return progress.map { "Downloading · \(Int($0 * 100))%" } ?? "Downloading…"
        }
        if let progress, row.status == .uploading {
            return "Sending · \(Int(progress * 100))%"
        }
        return row.statusLabel
    }

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: symbol)
                .frame(width: 26, height: 26)
                .foregroundStyle(row.status == .ready && received ? Color.accentColor : Color.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(row.title).lineLimit(1)
                Text([row.party, row.sizeLabel].filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(status)
                    .font(.caption)
                    .foregroundStyle(row.status == .ready && received ? Color.accentColor : Color.secondary)
                if let note = row.note, !note.isEmpty {
                    Text("“\(note)”")
                        .font(.caption)
                        .italic()
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
                if let progress, busy || row.status == .uploading {
                    ProgressView(value: progress)
                        .accessibilityLabel(busy ? "Download progress" : "Send progress")
                }
            }
            Spacer(minLength: 0)
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

/// " · 40%", or nothing if there is nothing to say yet.
private func percent(done: UInt64, total: UInt64) -> String {
    guard total > 0, done > 0 else { return "" }
    return " · \(Int(Double(done) / Double(total) * 100))%"
}

/// A picked file on its way up.
private struct UploadRow: View {
    let model: SessionModel
    let upload: QueuedUpload

    private var status: String {
        let shown = percent(done: upload.done, total: upload.total)
        return switch upload.state {
        case .waiting: "Waiting to upload to \(upload.folderName)\(shown)"
        case .preparing: "Preparing to upload to \(upload.folderName)"
        case .uploading: "Uploading to \(upload.folderName)\(shown)"
        case .offline: "Waiting for the connection to upload to \(upload.folderName)\(shown)"
        case .paused: "Paused upload to \(upload.folderName)\(shown)"
        case .failed: upload.problem ?? "This file couldn’t be uploaded"
        }
    }

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "arrow.up.doc")
                .frame(width: 26, height: 26)
            VStack(alignment: .leading, spacing: 2) {
                Text(upload.name).lineLimit(1)
                Text(status)
                    .font(.caption)
                    .foregroundStyle(upload.state == .failed ? Color.red : Color.secondary)
            }
            Spacer()
            if model.canRetry(upload) {
                Button(upload.state == .paused ? "Continue" : "Retry") { model.retryUpload(upload: upload) }
                    .buttonStyle(.borderless)
            }
            Button("Cancel Upload", systemImage: "xmark.circle.fill") {
                model.cancelUpload(upload: upload)
            }
            .labelStyle(.iconOnly)
            .foregroundStyle(.secondary)
            .buttonStyle(.borderless)
        }
    }
}

/// The uploads as one row in every folder, so that a failure can't go unseen. Opens Transfers.
struct UploadsSummaryRow: View {
    let uploads: [QueuedUpload]

    private var failed: Int { uploads.count { $0.state == .failed } }
    private var paused: [QueuedUpload] { uploads.filter { $0.state == .paused } }
    private var going: [QueuedUpload] { uploads.filter { $0.state != .failed && $0.state != .paused } }

    private var title: String {
        if going.isEmpty, failed == 0 {
            return paused.count == 1 ? "Upload of \(paused[0].name) paused" : "\(paused.count) uploads paused"
        }
        if going.isEmpty {
            return failed == 1 ? "1 file couldn’t be uploaded" : "\(failed) files couldn’t be uploaded"
        }
        return going.count == 1 ? "Uploading \(going[0].name)" : "Uploading \(going.count) files"
    }

    private var detail: String {
        var parts: [String] = []
        if !going.isEmpty, failed > 0 {
            parts.append("\(failed) failed")
        }
        if !paused.isEmpty, !going.isEmpty || failed > 0 {
            parts.append("\(paused.count) paused")
        }
        if going.contains(where: { $0.state == .offline }) {
            parts.append("Waiting for the connection")
        } else {
            let shown = going.isEmpty && failed == 0 ? paused : going
            let total = shown.reduce(0) { $0 + $1.total }
            let done = shown.reduce(0) { $0 + $1.done }
            if total > 0 {
                parts.append("\(Int(Double(done) / Double(total) * 100))%")
            }
        }
        return parts.joined(separator: " · ")
    }

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "arrow.up.doc")
                .frame(width: 26, height: 26)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).lineLimit(1)
                if !detail.isEmpty {
                    Text(detail)
                        .font(.caption)
                        .foregroundStyle(failed > 0 ? Color.red : Color.secondary)
                }
            }
            Spacer()
            Image(systemName: "chevron.right")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.tertiary)
        }
        .contentShape(Rectangle())
    }
}
