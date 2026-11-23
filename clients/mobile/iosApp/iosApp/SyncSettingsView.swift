import Photos
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// The Settings tab: the account, sync, and signing out.
struct SettingsView: View {
    let model: SessionModel
    let client: CoreClient
    let user: SignedInUser
    @State private var checksForUpdates = UpdatePrefs.checks

    var body: some View {
        Form {
            Section {
                NavigationLink {
                    ProfileView(model: model, client: client, user: user)
                } label: {
                    HStack(spacing: 12) {
                        Image(systemName: "person.crop.circle.fill")
                            .font(.largeTitle)
                            .foregroundStyle(Color.accentColor)
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(user.username).font(.headline)
                            Text("Profile and Devices")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
            Section("Files") {
                NavigationLink("Recycle Bin") {
                    TrashView(model: model, client: client, user: user)
                }
            }
            Section {
                NavigationLink("Sync") {
                    SyncSettingsView(model: model, client: client)
                }
            }
            Section {
                Toggle(updateLabels().checkSetting, isOn: $checksForUpdates)
                    .onChange(of: checksForUpdates) { _, on in UpdatePrefs.checks = on }
                LabeledContent("Version", value: appVersion())
            } header: {
                Text("About")
            } footer: {
                Text(updateLabels().checkSettingHint)
            }
            Section {
                Button("Sign Out", role: .destructive) { model.requestSignOut(client: client) }
            }
        }
        .navigationTitle("Settings")
    }
}

extension UploadSource: Identifiable {}

/// A local source picked, whose server folder is still to be chosen.
private struct PickedSource: Identifiable {
    let kind: SourceKind
    let handle: String
    let label: String
    var id: String { handle }
}

/// The two sync settings, and the local folders that upload to the server.
struct SyncSettingsView: View {
    let model: SessionModel
    let client: CoreClient
    @State private var settings = SyncSettings(wifiOnly: true, background: false)
    @State private var sources: [UploadSource] = []
    @State private var offline: OfflineStatus?
    @State private var error: String?
    @State private var addingSource = false
    @State private var importingFolder = false
    @State private var picked: PickedSource?
    @State private var changing: UploadSource?
    @State private var removing: UploadSource?
    @Environment(\.openURL) private var openURL

    private func offlineSummary(_ status: OfflineStatus) -> String {
        if status.unreachable {
            return "Can't reach the server. Trying again by itself."
        }
        if status.pending > 0 {
            return "\(status.pending) waiting to sync"
        }
        return status.kept == 0
            ? "Files you open and folders you make available offline are kept here."
            : "Up to date"
    }

    private var auto: AutoUpload? {
        try? ClientHolder.shared.active?.autoUpload()
    }

    var body: some View {
        Form {
            Section {
                Toggle("Sync only on Wi-Fi", isOn: binding(\.wifiOnly))
                Toggle("Sync in background", isOn: binding(\.background))
            } footer: {
                if settings.background, UIApplication.shared.backgroundRefreshStatus != .available {
                    backgroundRefreshHint
                } else {
                    Text(
                        "In the background, iOS decides when Home Space runs. Uploads may wait "
                            + "until the phone is charging."
                    )
                }
            }
            if let offline {
                Section("Offline Files") {
                    Text(offlineSummary(offline)).foregroundStyle(.secondary)
                    ForEach(offline.problems, id: \.name) { problem in
                        VStack(alignment: .leading) {
                            Text(problem.name)
                            Text(problem.reason).font(.caption).foregroundStyle(.red)
                        }
                    }
                    if !offline.problems.isEmpty {
                        Button("Try Again", systemImage: "arrow.clockwise") {
                            model.foregroundSync?.syncNow()
                        }
                    }
                }
            }
            Section {
                ForEach(sources, id: \.id) { source in
                    sourceRow(source)
                }
                Button("Add Folder", systemImage: "plus") { addingSource = true }
                if !sources.isEmpty {
                    Button("Sync Now", systemImage: "arrow.triangle.2.circlepath") {
                        model.foregroundSync?.syncNow()
                    }
                }
            } header: {
                Text("Folders That Upload")
            } footer: {
                Text("New and changed files go up. Changes on the server never affect this iPhone.")
            }
            if model.foregroundSync?.waitingForWifi == true, !sources.isEmpty {
                Text("Waiting for Wi-Fi").foregroundStyle(.secondary)
            }
            if let error {
                Text(error).foregroundStyle(.red)
            }
        }
        .navigationTitle("Sync")
        .onAppear(perform: load)
        .onChange(of: ClientHolder.shared.changes.generation) { load() }
        .confirmationDialog("What should upload?", isPresented: $addingSource, titleVisibility: .visible) {
            Button("Photo Library") { askForPhotos() }
            Button("Choose a Folder") { importingFolder = true }
        }
        .fileImporter(isPresented: $importingFolder, allowedContentTypes: [.folder]) { result in
            folderPicked(result)
        }
        .sheet(item: $picked) { source in
            ServerFolderSheet(
                title: "Upload “\(source.label)” to",
                confirm: "Add",
                askUploadExisting: true,
                model: model,
                client: client
            ) { folder, label, uploadExisting in
                add(source, folder: folder, label: label, uploadExisting: uploadExisting)
            }
        }
        .sheet(item: $changing) { source in
            ServerFolderSheet(
                title: "Upload “\(source.label)” to",
                confirm: "Change",
                askUploadExisting: false,
                model: model,
                client: client
            ) { folder, label, _ in
                attempt {
                    try auto?.setSourceTarget(id: source.id, targetId: folder, targetLabel: label)
                }
                load()
                model.foregroundSync?.syncNow()
            }
        }
        .alert(
            "Stop uploading “\(removing?.label ?? "")”?",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            presenting: removing
        ) { source in
            Button("Stop Uploading", role: .destructive) { remove(source) }
        } message: { _ in
            Text("Files already uploaded stay on the server. This iPhone isn't changed.")
        }
    }

    private var backgroundRefreshHint: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Background App Refresh is off for Home Space, so nothing runs in the background.")
            Button("Open Settings") {
                if let url = URL(string: UIApplication.openSettingsURLString) {
                    openURL(url)
                }
            }
        }
    }

    private func sourceRow(_ source: UploadSource) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(source.label)
            Text("To \(source.targetLabel)").font(.footnote).foregroundStyle(.secondary)
            Text(status(of: source))
                .font(.footnote)
                .foregroundStyle(source.problem != nil || source.lastError != nil ? .red : .secondary)
        }
        .swipeActions {
            Button("Remove", role: .destructive) { removing = source }
            Button("Change Folder") { changing = source }
        }
        .contextMenu {
            Button("Change Folder", systemImage: "folder") { changing = source }
            Button("Remove", systemImage: "trash", role: .destructive) { removing = source }
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
        if let date = source.lastSyncedAt {
            return "Up to date, \(localDate(date))"
        }
        return "Waiting to sync"
    }

    private func binding(_ key: WritableKeyPath<SyncSettings, Bool>) -> Binding<Bool> {
        Binding(
            get: { settings[keyPath: key] },
            set: { value in
                settings[keyPath: key] = value
                attempt { try auto?.setSettings(settings: settings) }
                BackgroundSync.schedule()
                model.foregroundSync?.syncNow()
            }
        )
    }

    private func load() {
        Task { offline = try? await client.offlineStatus() }
        guard let auto else { return }
        attempt {
            settings = try auto.settings()
            sources = try auto.sources()
        }
    }

    private func attempt(_ work: () throws -> Void) {
        do {
            try work()
        } catch {
            self.error = fileMessage(for: error)
        }
    }

    private func askForPhotos() {
        Task {
            let status = await PHPhotoLibrary.requestAuthorization(for: .readWrite)
            if status == .authorized || status == .limited {
                model.foregroundSync?.observePhotos()
                picked = PickedSource(kind: .photos, handle: photoLibraryHandle, label: "Photo Library")
            } else {
                error = "Home Space needs access to your photos to upload them. "
                    + "You can allow it in Settings."
            }
        }
    }

    private func folderPicked(_ result: Result<URL, Error>) {
        do {
            let url = try result.get()
            guard url.startAccessingSecurityScopedResource() else {
                error = "Home Space can't use that folder."
                return
            }
            defer { url.stopAccessingSecurityScopedResource() }
            let bookmark = try url.bookmarkData()
            picked = PickedSource(
                kind: .folder,
                handle: bookmark.base64EncodedString(),
                label: url.lastPathComponent
            )
        } catch {
            self.error = fileMessage(for: error)
        }
    }

    private func add(_ source: PickedSource, folder: String?, label: String, uploadExisting: Bool) {
        attempt {
            _ = try auto?.addSource(
                kind: source.kind,
                handle: source.handle,
                label: source.label,
                targetId: folder,
                targetLabel: label,
                uploadExisting: uploadExisting
            )
        }
        load()
        model.foregroundSync?.syncNow()
    }

    private func remove(_ source: UploadSource) {
        Task {
            do {
                try await auto?.removeSource(id: source.id)
            } catch {
                self.error = fileMessage(for: error)
            }
            load()
            model.foregroundSync?.syncNow()
        }
    }
}

/// Chooses a folder on the server; `folder` is `nil` for Home.
struct ServerFolderSheet: View {
    private struct Crumb: Equatable {
        let id: String
        let name: String
    }

    let title: String
    let confirm: String
    let askUploadExisting: Bool
    let model: SessionModel
    let client: CoreClient
    let onChoose: (_ folder: String?, _ label: String, _ uploadExisting: Bool) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var trail: [Crumb] = []
    @State private var folders: [Entry]?
    @State private var error: String?
    /// Off, so that adding a full photo library sends nothing by surprise.
    @State private var uploadExisting = false

    private var location: String {
        trail.isEmpty ? "Home" : "/" + trail.map(\.name).joined(separator: "/")
    }

    var body: some View {
        NavigationStack {
            List {
                Section(location) {
                    if !trail.isEmpty {
                        Button("Up", systemImage: "arrow.up") { trail.removeLast() }
                    }
                    ForEach(folders ?? [], id: \.id) { folder in
                        Button(folder.name, systemImage: "folder") {
                            trail.append(Crumb(id: folder.id, name: folder.name))
                        }
                    }
                    if folders?.isEmpty == true {
                        Text("No Subfolders").foregroundStyle(.secondary)
                    }
                }
                if askUploadExisting {
                    Section {
                        Toggle("Upload Existing Files", isOn: $uploadExisting)
                    } footer: {
                        Text("When off, only files added or changed from now on are uploaded.")
                    }
                }
                if let error {
                    Text(error).foregroundStyle(.red)
                }
            }
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(confirm) {
                        onChoose(trail.last?.id, trail.last?.name ?? "Home", uploadExisting)
                        dismiss()
                    }
                }
            }
        }
        .task(id: trail.last?.id) { await load() }
    }

    private func load() async {
        folders = nil
        do {
            folders = try await client.list(parentId: trail.last?.id).filter { $0.kind == .folder }
        } catch is CancellationError {
            // A newer load took over.
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }
}
