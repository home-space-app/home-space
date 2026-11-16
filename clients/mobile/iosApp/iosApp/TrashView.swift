import SwiftUI

/// What asks before deleting for good: some files, or the whole bin.
private enum ForeverRequest: Identifiable {
    case files([TrashEntry])
    case all

    var id: String {
        switch self {
        case .files(let entries): entries.map(\.id).joined(separator: ",")
        case .all: "all"
        }
    }
}

/// Restored files whose names were taken in Lost & Found, waiting for the user's choice.
private struct PendingRestore: Identifiable {
    let ids: [String]
    let result: RestoreResult
    var id: String { ids.joined(separator: ",") }
}

/// The files the user deleted, to restore to Lost & Found or delete for good. Pushed from
/// Settings; like the Files app's Recently Deleted.
struct TrashView: View {
    let model: SessionModel
    let client: CoreClient
    let user: SignedInUser
    @State private var entries: [TrashEntry]?
    @State private var loadError: String?
    @State private var selection = Set<String>()
    @State private var editMode = EditMode.inactive
    @State private var forever: ForeverRequest?
    @State private var pending: PendingRestore?
    @State private var working = false
    @State private var actionError: String?
    @State private var restored: String?
    @State private var reload = 0

    private var selecting: Bool { editMode.isEditing }

    var body: some View {
        content
            .navigationTitle("Recycle Bin")
            .navigationBarTitleDisplayMode(.large)
            .toolbar { toolbar }
            .environment(\.editMode, $editMode)
            // The selection's toolbar takes the tab bar's place, as in Files.
            .toolbar(selecting ? .hidden : .visible, for: .tabBar)
            .task(id: "\(model.trashGeneration)-\(reload)") { await load() }
            .refreshable { await load() }
            .confirmationDialog(
                foreverTitle,
                isPresented: Binding(get: { forever != nil }, set: { if !$0 { forever = nil } }),
                titleVisibility: .visible,
                presenting: forever
            ) { request in
                Button("Delete", role: .destructive) { deleteForever(request) }
                Button("Cancel", role: .cancel) {}
            } message: { _ in
                Text("This can't be undone.")
            }
            .sheet(item: $pending) { found in
                NavigationStack {
                    ConflictList(
                        title: found.result.conflictsTitle,
                        conflicts: found.result.conflicts,
                        saving: working,
                        error: actionError,
                        onResolve: { mode in restore(found.ids, mode: mode) },
                        onCancel: {
                            pending = nil
                            actionError = nil
                        }
                    )
                    .navigationBarTitleDisplayMode(.inline)
                }
                .interactiveDismissDisabled(working)
            }
            .alert(
                "Couldn't Finish",
                isPresented: Binding(
                    get: { actionError != nil && pending == nil },
                    set: { if !$0 { actionError = nil } }
                )
            ) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(actionError ?? "")
            }
            .safeAreaInset(edge: .bottom) {
                if let restored, !selecting {
                    RestoredBanner(message: restored) {
                        self.restored = nil
                        AppRouter.shared.openFolder = FolderRoute(id: user.lostAndFoundId, title: "Lost & Found")
                        AppRouter.shared.tab = .files
                    }
                    .task(id: restored) {
                        try? await Task.sleep(for: .seconds(5))
                        if self.restored == restored { self.restored = nil }
                    }
                }
            }
            .animation(.default, value: restored)
    }

    @ViewBuilder
    private var content: some View {
        if let entries {
            if entries.isEmpty {
                ContentUnavailableView(
                    "No Deleted Files",
                    systemImage: "trash",
                    description: Text("Deleted files stay here until you delete them forever.")
                )
            } else {
                list(entries)
            }
        } else if let loadError {
            ContentUnavailableView {
                Label("Can't Show the Recycle Bin", systemImage: "exclamationmark.triangle")
            } description: {
                Text(loadError)
            } actions: {
                Button("Try Again") { reload += 1 }
            }
        } else {
            ProgressView()
        }
    }

    private func list(_ entries: [TrashEntry]) -> some View {
        // Rows are only selectable in Select mode; a tap outside it would leave one highlighted.
        List(selection: selecting ? $selection : nil) {
            Section {
                ForEach(entries, id: \.id) { entry in
                    TrashRow(entry: entry)
                        .swipeActions(edge: .leading) {
                            Button {
                                restore([entry.id], mode: .abort)
                            } label: {
                                Label("Restore", systemImage: "arrow.uturn.backward")
                            }
                            .tint(.accentColor)
                        }
                        .swipeActions(edge: .trailing) {
                            // Asks first, so the row is not removed by the swipe itself.
                            Button {
                                forever = .files([entry])
                            } label: {
                                Label("Delete", systemImage: "trash")
                            }
                            .tint(.red)
                        }
                        .contextMenu {
                            Button {
                                restore([entry.id], mode: .abort)
                            } label: {
                                Label("Restore", systemImage: "arrow.uturn.backward")
                            }
                            Button(role: .destructive) {
                                forever = .files([entry])
                            } label: {
                                Label("Delete Forever", systemImage: "trash")
                            }
                        }
                }
            } header: {
                Text("Restored files go to Lost & Found.")
                    .textCase(nil)
            }
        }
        .disabled(working)
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            if !(entries?.isEmpty ?? true) {
                Button(selecting ? "Done" : "Select") {
                    withAnimation {
                        editMode = selecting ? .inactive : .active
                        selection = []
                    }
                }
                .fontWeight(selecting ? .semibold : .regular)
            }
        }
        if selecting, let entries {
            let chosen = entries.filter { selection.contains($0.id) }
            ToolbarItemGroup(placement: .bottomBar) {
                Button(chosen.isEmpty ? "Restore All" : "Restore") {
                    restore((chosen.isEmpty ? entries : chosen).map(\.id), mode: .abort)
                }
                .disabled(working)
                Spacer()
                Button(chosen.isEmpty ? "Delete All" : "Delete", role: .destructive) {
                    forever = chosen.isEmpty ? .all : .files(chosen)
                }
                .tint(.red)
                .disabled(working)
            }
        }
    }

    private var foreverTitle: String {
        switch forever {
        case .files(let chosen) where chosen.count == 1:
            "Delete “\(chosen[0].name)” Forever?"
        case .files(let chosen):
            "Delete \(chosen.count) Files Forever?"
        case .all:
            "Delete \(entries?.count ?? 0) Files Forever?"
        case nil:
            ""
        }
    }

    private func load() async {
        do {
            let found = try await client.trash()
            entries = found
            loadError = nil
            // A file that is gone can't stay selected.
            selection.formIntersection(found.map(\.id))
            if found.isEmpty { editMode = .inactive }
        } catch is CancellationError {
            // A newer load took over.
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            loadError = fileMessage(for: error)
        }
    }

    private func restore(_ ids: [String], mode: MoveMode) {
        working = true
        actionError = nil
        Task {
            defer { working = false }
            do {
                let result = try await client.restoreTrashed(ids: ids, mode: mode)
                if !result.conflicts.isEmpty {
                    pending = PendingRestore(ids: ids, result: result)
                    return
                }
                pending = nil
                editMode = .inactive
                selection = []
                if !result.message.isEmpty { restored = result.message }
                reload += 1
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                actionError = fileMessage(for: error)
            }
        }
    }

    private func deleteForever(_ request: ForeverRequest) {
        working = true
        Task {
            defer { working = false }
            do {
                switch request {
                case .files(let chosen): try await client.deleteForever(ids: chosen.map(\.id))
                case .all: try await client.emptyTrash()
                }
                editMode = .inactive
                selection = []
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                actionError = fileMessage(for: error)
            }
            reload += 1
        }
    }
}

/// A binned file: its name, and where it was, when it went and how big it is.
private struct TrashRow: View {
    let entry: TrashEntry

    var body: some View {
        HStack(spacing: 12) {
            EntryIconView(icon: entry.icon, state: nil)
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.name).lineLimit(1)
                Text(
                    "\(entry.folder) · \(entry.deletedAt.formatted(date: .abbreviated, time: .omitted)) · \(entry.sizeLabel)"
                )
                .font(.subheadline)
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .lineLimit(1)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// Says where restored files went, with the way there. Goes by itself after a while.
private struct RestoredBanner: View {
    let message: String
    let onShow: () -> Void

    var body: some View {
        HStack {
            Text(message)
                .font(.subheadline)
            Spacer()
            Button("Show", action: onShow)
                .font(.subheadline.weight(.semibold))
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(.regularMaterial, in: .rect(cornerRadius: 14))
        .shadow(color: .black.opacity(0.12), radius: 8, y: 2)
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }
}
