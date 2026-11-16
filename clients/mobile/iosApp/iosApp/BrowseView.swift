import PhotosUI
import QuickLook
import SwiftUI

/// The height of a row, measured from the Files app on an iPhone. The Android client uses the same.
private let rowHeight: CGFloat = 64

/// The look of the app's file lists, taken from the Files app: end to end, with tall rows.
extension View {
    func fileList() -> some View {
        listStyle(.plain).environment(\.defaultMinListRowHeight, rowHeight)
    }

    /// Little vertical padding, so that two lines of text still fit the row height.
    func fileRow() -> some View {
        listRowInsets(EdgeInsets(top: 6, leading: 20, bottom: 6, trailing: 20))
    }
}

/// A folder pushed in the Files tab; a `nil` id is Home.
struct FolderRoute: Hashable {
    let id: String?
    let title: String
}

/// Folder browsing through the Rust client (`clients/mobile-core`).
struct BrowseView: View {
    let model: SessionModel
    let client: CoreClient
    let user: SignedInUser
    @State private var path: [FolderRoute] = []

    var body: some View {
        NavigationStack(path: $path) {
            FolderView(model: model, client: client, user: user, folderId: nil, title: "Home", path: $path)
                .navigationDestination(for: FolderRoute.self) { route in
                    FolderView(
                        model: model, client: client, user: user, folderId: route.id, title: route.title,
                        path: $path)
                }
        }
    }
}

private struct FolderView: View {
    let model: SessionModel
    let client: CoreClient
    let user: SignedInUser
    let folderId: String?
    let title: String
    @Binding var path: [FolderRoute]
    @State private var entries: [Entry] = []
    @State private var error: String?
    @State private var selection = Set<String>()
    @State private var editMode = EditMode.inactive
    @State private var action: FileAction?
    @State private var query = ""
    @State private var results: [SearchResult] = []
    /// Whether what `results` answers is the query typed now, so "no results" isn't shown early.
    @State private var answered = false
    @State private var pickingFiles = false
    @State private var pickingPhotos = false
    @State private var photos: [PhotosPickerItem] = []
    @State private var pickingSendFiles = false
    @State private var pickingSendPhotos = false
    @State private var sendPhotos: [PhotosPickerItem] = []
    /// Files picked to send, waiting for the picker to close before the device sheet opens.
    @State private var pendingSend: SendPayload?
    @State private var preparingSend = false

    private var searchText: String { query.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var searching: Bool { !searchText.isEmpty }
    private var selecting: Bool { editMode.isEditing }
    private var chosen: [Entry] { entries.filter { selection.contains($0.id) } }

    var body: some View {
        List(selection: $selection) {
            if let error {
                Text(error).foregroundStyle(.red)
            }
            if searching {
                ForEach(results, id: \.entry.id) { result in
                    row(result.entry, location: result.location)
                        .contextMenu {
                            if result.entry.kind == .file {
                                Button("Show in Folder", systemImage: "folder") {
                                    showInFolder(result.entry)
                                }
                            }
                            rowActions(result.entry)
                        }
                        .fileRow()
                        .listRowBackground(ProgressBackground(model: model, id: result.entry.id))
                }
            } else {
                if !model.uploads.isEmpty {
                    Button {
                        AppRouter.shared.tab = .transfers
                    } label: {
                        UploadsSummaryRow(uploads: model.uploads)
                    }
                    .buttonStyle(.plain)
                    .selectionDisabled()
                    .fileRow()
                }
                ForEach(entries, id: \.id) { entry in
                    row(entry)
                        .contextMenu {
                            if !selecting {
                                rowActions(entry)
                            }
                        }
                        .fileRow()
                        .listRowBackground(ProgressBackground(model: model, id: entry.id))
                }
            }
        }
        .overlay {
            if searching && answered && results.isEmpty && error == nil {
                ContentUnavailableView.search(text: searchText)
            }
        }
        .fileList()
        .navigationTitle(selecting ? "\(selection.count) selected" : title)
        .navigationBarBackButtonHidden(selecting)
        .environment(\.editMode, $editMode)
        .searchable(text: $query, prompt: "Search Files and Folders")
        .onChange(of: searchText) {
            answered = false
            if searching && selecting {
                selection = []
                editMode = .inactive
            }
        }
        .toolbar {
            if selecting {
                ToolbarItem(placement: .topBarLeading) {
                    if selection.count < entries.count {
                        Button("Select All") { selection = Set(entries.map(\.id)) }
                    } else {
                        Button("Deselect All") { selection = [] }
                    }
                }
            }
            if !selecting {
                ToolbarItem(placement: .topBarTrailing) { addMenu }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button(selecting ? "Done" : "Select") {
                    selection = []
                    editMode = selecting ? .inactive : .active
                }
                .disabled(entries.isEmpty || searching)
            }
            if preparingSend {
                ToolbarItem(placement: .topBarLeading) { ProgressView() }
            }
            if selecting && !chosen.isEmpty {
                ToolbarItemGroup(placement: .bottomBar) { selectionActions }
            }
        }
        .toolbar(selecting && !chosen.isEmpty ? .visible : .hidden, for: .bottomBar)
        .sheet(item: $action) { current in
            FileDialog(action: current, model: model, client: client, owner: user.username) {
                action = nil
                selection = []
                editMode = .inactive
                model.reload()
            }
        }
        .sheet(isPresented: $pickingFiles) {
            ImportPicker { files in
                model.uploadFiles(files: files, parentId: folderId, folderName: title)
            }
            .ignoresSafeArea()
        }
        .photosPicker(
            isPresented: $pickingPhotos,
            selection: $photos,
            matching: .any(of: [.images, .videos]),
            preferredItemEncoding: .current
        )
        .onChange(of: photos) {
            guard !photos.isEmpty else { return }
            model.uploadPhotos(photos: photos, parentId: folderId, folderName: title)
            photos = []
        }
        .sheet(
            isPresented: $pickingSendFiles,
            onDismiss: {
                if let pendingSend {
                    action = .send(pendingSend)
                    self.pendingSend = nil
                }
            }
        ) {
            ImportPicker { files in
                do {
                    pendingSend = try TransferCenter.prepare(files: files)
                } catch {
                    model.notice = fileMessage(for: error)
                }
            }
            .ignoresSafeArea()
        }
        .photosPicker(
            isPresented: $pickingSendPhotos,
            selection: $sendPhotos,
            matching: .any(of: [.images, .videos]),
            preferredItemEncoding: .current
        )
        .onChange(of: sendPhotos) {
            guard !sendPhotos.isEmpty else { return }
            let picked = sendPhotos
            sendPhotos = []
            Task { await prepareSend(picked) }
        }
        .task(id: model.reloadKey) { await load() }
        // Changes from other devices, when they touch this folder.
        .onChange(of: model.liveChanges) { _, changes in
            if changes.touches(folder: folderId, entries: entries) {
                Task { await load() }
            }
        }
        .task(id: SearchRequest(text: searchText, reloadKey: model.reloadKey)) { await search() }
        // Updating and failed copies change while the folder is open, with nothing new on the server.
        .onChange(of: ClientHolder.shared.changes.generation) {
            Task {
                let marked = await client.refreshOffline(entries: entries)
                if marked.map(\.id) == entries.map(\.id) {
                    entries = marked
                }
            }
        }
    }

    private var addMenu: some View {
        Menu("Add", systemImage: "plus") {
            Button("New Folder", systemImage: "folder.badge.plus") { action = .newFolder(folderId) }
            Button("Upload Files", systemImage: "doc.badge.plus") { pickingFiles = true }
            Button("Upload Photos", systemImage: "photo.badge.plus") { pickingPhotos = true }
            Section {
                Button("Send Files to a Device", systemImage: "paperplane") { pickingSendFiles = true }
                Button("Send Photos to a Device", systemImage: "photo.on.rectangle") { pickingSendPhotos = true }
            }
        }
    }

    @ViewBuilder
    private func row(_ entry: Entry, location: String? = nil) -> some View {
        switch entry.kind {
        case .folder:
            NavigationLink(value: FolderRoute(id: entry.id, title: entry.name)) {
                rowLabel(entry, location: location)
            }
        case .file:
            if selecting {
                rowLabel(entry, location: location)
            } else {
                Button { model.openFile(client: client, entry: entry) } label: {
                    rowLabel(entry, location: location)
                }
                    .buttonStyle(.plain)
            }
        }
    }

    private func rowLabel(_ entry: Entry, location: String?) -> some View {
        HStack(spacing: 12) {
            EntryIconView(icon: entry.icon, state: entry.offlineState)
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.name)
                if let location {
                    Text(location)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else if entry.kind == .file {
                    Text("\(entry.sizeLabel) · \(localDate(entry.updatedAt))")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }

    /// What a long press on a row offers.
    @ViewBuilder
    private func rowActions(_ entry: Entry) -> some View {
        let downloading = model.downloading.contains(entry.id)
        if downloading {
            Button("Cancel download", systemImage: "xmark.circle") {
                model.cancelDownload(client: client, entry: entry)
            }
        }
        if entry.kind == .file && !downloading {
            if viewerFor(mime: entry.mime, size: entry.size) != nil {
                Button("Open in Quick Look", systemImage: "eye") {
                    model.openInQuickLook(client: client, entry: entry)
                }
            }
            Button("Save to…", systemImage: "square.and.arrow.down") {
                model.saveFile(client: client, entry: entry)
            }
        }
        Section {
            Button("Share…", systemImage: "person.crop.circle.badge.plus") { action = .share([entry]) }
            Button("Send to Device…", systemImage: "paperplane") { action = .send(.entries([entry])) }
        }
        Button("Details", systemImage: "info.circle") { action = .details(entry) }
        Button("Rename", systemImage: "pencil") { action = .rename(entry) }
        Button("Move", systemImage: "folder") { action = .move([entry]) }
        if entry.offlineKept && !downloading {
            Button("Remove Offline Copy", systemImage: "arrow.down.circle.dotted") {
                model.removeOfflineCopy(client: client, entry: entry)
            }
        } else if entry.offlineState == nil && !downloading {
            Button("Make Available Offline", systemImage: "arrow.down.circle") {
                model.makeAvailableOffline(client: client, entry: entry)
            }
        }
        if entry.id != user.lostAndFoundId {
            Divider()
            Button("Delete", systemImage: "trash", role: .destructive) { action = .delete([entry]) }
        }
    }

    @ViewBuilder
    private var selectionActions: some View {
        if chosen.count == 1, let entry = chosen.first {
            Button("Rename") { action = .rename(entry) }
            Spacer()
            Button("Details") { action = .details(entry) }
            Spacer()
        }
        Button("Move") { action = .move(chosen) }
        if !chosen.contains(where: { $0.id == user.lostAndFoundId }) {
            Spacer()
            Button("Delete", role: .destructive) { action = .delete(chosen) }
        }
        Spacer()
        Menu("More", systemImage: "ellipsis.circle") {
            Button("Share…", systemImage: "person.crop.circle.badge.plus") { action = .share(chosen) }
            Button("Send to Device…", systemImage: "paperplane") { action = .send(.entries(chosen)) }
        }
    }

    /// Opens the folder a search result is in, with the folders above it to go back through.
    private func showInFolder(_ entry: Entry) {
        Task {
            do {
                let above = try await client.ancestors(id: entry.id)
                query = ""
                path = above.filter { $0.kind == .folder }.map { FolderRoute(id: $0.id, title: $0.name) }
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                model.notice = fileMessage(for: error)
            }
        }
    }

    /// Copies picked photos out of the library, then asks which device they go to.
    private func prepareSend(_ picked: [PhotosPickerItem]) async {
        preparingSend = true
        defer { preparingSend = false }
        do {
            let folder = try TransferCenter.sendFolder()
            var files: [SendFile] = []
            for item in picked {
                guard let photo = try await item.loadTransferable(type: PickedPhoto.self) else { continue }
                let target = freeURL(folder.appendingPathComponent(photo.url.lastPathComponent))
                try FileManager.default.moveItem(at: photo.url, to: target)
                try? FileManager.default.removeItem(at: photo.url.deletingLastPathComponent())
                files.append(SendFile(name: target.lastPathComponent, path: target.path, mime: TransferCenter.mime(of: target)))
            }
            guard !files.isEmpty else {
                try? FileManager.default.removeItem(at: folder)
                return
            }
            action = .send(.files(files, folder: folder))
        } catch {
            model.notice = "Could not read the photos: \(fileMessage(for: error))"
        }
    }

    /// Waits for typing to pause, as the desktop client does. A new request cancels this one, so a
    /// late answer can't replace a newer one.
    private func search() async {
        guard searching else {
            results = []
            return
        }
        do {
            try await Task.sleep(for: .milliseconds(250))
            let found = try await client.search(query: searchText)
            try Task.checkCancellation()
            error = nil
            results = found
            answered = true
        } catch is CancellationError {
            // A newer search took over.
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }

    private func load() async {
        do {
            let listed = try await client.list(parentId: folderId)
            error = nil
            entries = listed
            // A file that is gone can't stay selected.
            selection.formIntersection(listed.map(\.id))
        } catch is CancellationError {
            // A newer load took over.
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }
}

/// What makes the search run again.
private struct SearchRequest: Equatable {
    let text: String
    let reloadKey: Int
}

/// A file on its way up, shown in every folder so that a failure can't go unseen.
/// A row's background: it fills from the leading edge as the file downloads.
private struct ProgressBackground: View {
    let model: SessionModel
    let id: String

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Color(.systemBackground)
                if let progress = model.progress[id] {
                    Color("FolderAccent")
                        .opacity(0.25)
                        .frame(width: geometry.size.width * progress)
                }
            }
        }
    }
}

/// The same icons and colours as the web client: only folders are coloured.
struct EntryIconView: View {
    let icon: EntryIcon
    let state: OfflineState?

    private var asset: String {
        switch icon {
        case .folder: "node-folder"
        case .image: "node-image"
        case .document: "node-description"
        case .other: "node-draft"
        }
    }

    var body: some View {
        // The box and badge sizes are the desktop client's.
        ZStack(alignment: .leading) {
            Image(decorative: asset)
                .renderingMode(.template)
                .resizable()
                .frame(width: 20, height: 20)
                .foregroundStyle(icon == .folder ? Color("FolderAccent") : Color.primary)
            if let state {
                badge(for: state)
                    .frame(width: 12, height: 12)
                    .padding(1)
                    .background(Color(.systemBackground), in: Circle())
                    .frame(width: 26, height: 26, alignment: .bottomTrailing)
            }
        }
        .frame(width: 26, height: 26)
    }

    @ViewBuilder
    private func badge(for state: OfflineState) -> some View {
        switch state {
        case .synced:
            Image(decorative: "sync-check")
                .renderingMode(.template)
                .resizable()
                .foregroundStyle(Color("SyncedBadge"))
                .accessibilityLabel("Available offline")
        case .updating:
            Image(systemName: "arrow.triangle.2.circlepath")
                .resizable()
                .scaledToFit()
                .foregroundStyle(Color("FolderAccent"))
                .accessibilityLabel("Syncing")
        case .failed:
            Image(systemName: "exclamationmark.circle.fill")
                .resizable()
                .foregroundStyle(.red)
                .accessibilityLabel("Sync failed")
        }
    }
}
