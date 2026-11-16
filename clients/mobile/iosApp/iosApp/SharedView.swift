import SwiftUI

/// What other users shared with this one: the shares at the top, then the folders inside them.
struct SharedView: View {
    let model: SessionModel
    let client: CoreClient

    var body: some View {
        SharedFolderView(model: model, client: client, folder: nil)
            .navigationDestination(for: SharedEntry.self) { folder in
                SharedFolderView(model: model, client: client, folder: folder)
            }
    }
}

private struct SharedFolderView: View {
    let model: SessionModel
    let client: CoreClient
    /// `nil` for everything shared with the user.
    let folder: SharedEntry?
    @State private var entries: [SharedEntry]?
    @State private var error: String?
    @State private var copying: [SharedEntry]?

    var body: some View {
        List {
            if let error {
                Text(error).foregroundStyle(.red)
            }
            ForEach(entries ?? [], id: \.id) { entry in
                row(entry)
                    .contextMenu { actions(entry) }
                    .fileRow()
            }
        }
        .fileList()
        .overlay {
            if entries == nil && error == nil {
                ProgressView()
            } else if entries?.isEmpty == true && error == nil {
                if folder == nil {
                    ContentUnavailableView(
                        "Nothing Shared",
                        systemImage: "person.2",
                        description: Text("Files and folders other people share with you show up here.")
                    )
                } else {
                    ContentUnavailableView("Empty Folder", systemImage: "folder")
                }
            }
        }
        .navigationTitle(folder?.name ?? "Shared with Me")
        .refreshable { await load() }
        .task { await load() }
        .onChange(of: model.liveChanges) { _, changes in
            if changes.resync {
                Task { await load() }
            }
        }
        .sheet(item: Binding(
            get: { copying.map(CopyRequest.init) },
            set: { if $0 == nil { copying = nil } }
        )) { request in
            ServerFolderSheet(
                title: "Save to My Files",
                confirm: "Save",
                askUploadExisting: false,
                model: model,
                client: client
            ) { parent, _, _ in
                copy(request.entries, to: parent)
            }
        }
    }

    @ViewBuilder
    private func row(_ entry: SharedEntry) -> some View {
        switch entry.kind {
        case .folder:
            NavigationLink(value: entry) { label(entry) }
        case .file:
            Button {
                model.openShared(client: client, entry: entry, siblings: entries ?? [])
            } label: {
                label(entry)
            }
            .buttonStyle(.plain)
            .listRowBackground(SharedProgress(model: model, id: entry.id))
        }
    }

    private func label(_ entry: SharedEntry) -> some View {
        HStack(spacing: 12) {
            EntryIconView(icon: entry.icon, state: nil)
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.name)
                Text(caption(entry))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }

    private func caption(_ entry: SharedEntry) -> String {
        var parts: [String] = []
        if folder == nil {
            parts.append("From \(entry.owner)")
        }
        if entry.kind == .file {
            parts.append(entry.sizeLabel)
        }
        if folder == nil, let expires = entry.expiresAt {
            parts.append("Until \(expires.formatted(date: .abbreviated, time: .omitted))")
        } else if folder != nil {
            parts.append(localDate(entry.updatedAt))
        }
        return parts.joined(separator: " · ")
    }

    @ViewBuilder
    private func actions(_ entry: SharedEntry) -> some View {
        if entry.kind == .file {
            Button("Open", systemImage: "eye") {
                model.openShared(client: client, entry: entry, siblings: entries ?? [])
            }
            Button("Save to…", systemImage: "square.and.arrow.down") {
                model.saveShared(client: client, entry: entry)
            }
        }
        Button("Save to My Files…", systemImage: "folder.badge.plus") { copying = [entry] }
    }

    private func copy(_ items: [SharedEntry], to parent: String?) {
        Task {
            do {
                try await client.copyShared(entries: items, parentId: parent)
                model.reload()
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                model.notice = "Could not save a copy: \(fileMessage(for: error))"
            }
        }
    }

    private func load() async {
        do {
            if let folder {
                entries = try await client.sharedChildren(folder: folder)
            } else {
                entries = try await client.sharedWithMe()
            }
            error = nil
        } catch is CancellationError {
            // A newer load took over.
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }
}

/// Shared items to copy into the user's files, once a folder is chosen.
private struct CopyRequest: Identifiable {
    let entries: [SharedEntry]
    var id: String { entries.map(\.id).joined() }
}

/// A row's background, filling as the shared file downloads.
private struct SharedProgress: View {
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
