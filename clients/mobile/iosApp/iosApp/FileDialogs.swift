import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// A time in the device's locale and time zone.
func localDate(_ time: Date) -> String {
    time.formatted(date: .abbreviated, time: .shortened)
}

private func describe(_ side: ConflictSide) -> String {
    "\(localDate(side.updatedAt)) · \(side.sizeLabel)"
}

/// The dialog the browser is showing.
enum FileAction: Identifiable {
    case details(Entry)
    case rename(Entry)
    case move([Entry])
    case delete([Entry])
    /// A new folder in the folder with this id, or Home when nil.
    case newFolder(String?)
    /// Sharing with people or by a link.
    case share([Entry])
    case send(SendPayload)

    var id: String {
        switch self {
        case .details(let entry): "details-\(entry.id)"
        case .rename(let entry): "rename-\(entry.id)"
        case .move(let entries): "move-\(entries.map(\.id).joined())"
        case .delete(let entries): "delete-\(entries.map(\.id).joined())"
        case .newFolder(let parent): "new-folder-\(parent ?? "")"
        case .share(let entries): "share-\(entries.map(\.id).joined())"
        case .send(let payload): "send-\(payload.id)"
        }
    }
}

/// Shows the dialog of `action`. `onDone` means something changed, so the folder is out of date.
struct FileDialog: View {
    let action: FileAction
    let model: SessionModel
    let client: CoreClient
    /// The signed-in user's name, which the share dialog lists first.
    var owner = ""
    let onDone: () -> Void

    var body: some View {
        switch action {
        case .share(let entries):
            ShareSheet(model: model, client: client, entries: entries, owner: owner)
        case .send(let payload):
            SendSheet(model: model, payload: payload, onSent: onDone)
        case .details(let entry):
            DetailsSheet(entry: entry, model: model, client: client)
        case .rename(let entry):
            NameSheet(
                title: "Rename “\(entry.name)”",
                confirm: "Rename",
                initial: entry.name,
                model: model,
                onDone: onDone,
                save: { _ = try await client.rename(id: entry.id, name: $0) }
            )
        case .newFolder(let parent):
            NameSheet(
                title: "New Folder",
                confirm: "Create",
                initial: "",
                model: model,
                onDone: onDone,
                save: { _ = try await client.createFolder(parentId: parent, name: $0) }
            )
        case .move(let entries):
            MoveSheet(entries: entries, model: model, client: client, onDone: onDone)
        case .delete(let entries):
            DeleteSheet(entries: entries, model: model, client: client, onDone: onDone)
        }
    }
}

private struct DetailsSheet: View {
    let entry: Entry
    let model: SessionModel
    let client: CoreClient
    @State private var details: EntryDetails?
    @State private var error: String?

    var body: some View {
        NavigationStack {
            List {
                if let error {
                    Text(error).foregroundStyle(.red)
                }
                if let details {
                    LabeledContent("Type", value: details.typeLabel)
                    if details.entry.kind == .file {
                        LabeledContent("Size", value: details.entry.sizeLabel)
                        LabeledContent("Version", value: String(details.entry.version))
                    }
                    LabeledContent("Created", value: localDate(details.createdAt))
                    LabeledContent("Modified", value: localDate(details.entry.updatedAt))
                    if let device = details.deviceLabel {
                        LabeledContent(details.deviceTitle, value: device)
                    }
                    if let source = details.sourceDevice {
                        LabeledContent("Uploaded from", value: source)
                    }
                }
            }
            .navigationTitle(entry.name)
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium, .large])
        .task {
            do {
                details = try await client.details(id: entry.id)
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                self.error = fileMessage(for: error)
            }
        }
    }
}

private struct NameSheet: View {
    let title: String
    let confirm: String
    let initial: String
    let model: SessionModel
    let onDone: () -> Void
    let save: (String) async throws -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var saving = false
    @State private var error: String?
    @FocusState private var focused: Bool

    init(
        title: String,
        confirm: String,
        initial: String,
        model: SessionModel,
        onDone: @escaping () -> Void,
        save: @escaping (String) async throws -> Void
    ) {
        self.title = title
        self.confirm = confirm
        self.initial = initial
        self.model = model
        self.onDone = onDone
        self.save = save
        _name = State(initialValue: initial)
    }

    var body: some View {
        NavigationStack {
            Form {
                TextField("Name", text: $name)
                    .focused($focused)
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .submitLabel(.done)
                    .onSubmit(submit)
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
                    Button(saving ? "Working…" : confirm, action: submit).disabled(saving)
                }
            }
        }
        .presentationDetents([.medium])
        // The name is preselected, so typing replaces it.
        .onReceive(NotificationCenter.default.publisher(for: UITextField.textDidBeginEditingNotification)) {
            let field = $0.object as? UITextField
            DispatchQueue.main.async { field?.selectAll(nil) }
        }
        .onAppear { focused = true }
    }

    private func submit() {
        let typed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !saving else { return }
        guard !typed.isEmpty, typed != initial else {
            dismiss()
            return
        }
        saving = true
        error = nil
        Task {
            do {
                try await save(typed)
                onDone()
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                self.error = fileMessage(for: error)
                saving = false
            }
        }
    }
}

private struct DeleteSheet: View {
    let entries: [Entry]
    let model: SessionModel
    let client: CoreClient
    let onDone: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var saving = false
    @State private var error: String?

    private var title: String {
        if entries.count == 1, let entry = entries.first {
            return "Move “\(entry.name)” to recycle bin?"
        }
        return "Move \(entries.count) items to recycle bin?"
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 12) {
                Text(title).font(.headline)
                if entries.contains(where: { $0.kind == .folder }) {
                    Text("Files inside folders are moved too. You can restore them from the recycle bin.")
                }
                if let error {
                    Text(error).foregroundStyle(.red)
                }
                Spacer()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding()
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(saving ? "Working…" : "Move to bin", role: .destructive, action: delete)
                        .disabled(saving)
                }
            }
        }
        .presentationDetents([.fraction(0.3), .medium])
    }

    private func delete() {
        saving = true
        error = nil
        Task {
            do {
                try await client.delete(ids: entries.map(\.id))
                onDone()
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                self.error = fileMessage(for: error)
                saving = false
            }
        }
    }
}

private struct MoveSheet: View {
    /// A folder opened on the way down from Home.
    private struct Crumb: Equatable {
        let id: String
        let name: String
    }

    let entries: [Entry]
    let model: SessionModel
    let client: CoreClient
    let onDone: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var trail: [Crumb] = []
    @State private var folders: [Entry]?
    @State private var conflicts: MoveResult?
    @State private var saving = false
    @State private var error: String?

    private var destination: String? { trail.last?.id }

    private var title: String {
        if entries.count == 1, let entry = entries.first {
            return "Move “\(entry.name)”"
        }
        return "Move \(entries.count) items"
    }

    private var location: String {
        trail.isEmpty ? "Home" : "/" + trail.map(\.name).joined(separator: "/")
    }

    var body: some View {
        NavigationStack {
            Group {
                if let conflicts {
                    conflictList(conflicts)
                } else {
                    picker
                }
            }
            .navigationBarTitleDisplayMode(.inline)
        }
        .task(id: destination) { await loadFolders() }
    }

    private var picker: some View {
        List {
            Section("To \(location)") {
                if !trail.isEmpty {
                    Button { trail.removeLast() } label: {
                        pickerRow {
                            Image(systemName: "arrow.up").frame(width: 26, height: 26)
                            Text("Up")
                        }
                    }
                    .buttonStyle(.plain)
                    .fileRow()
                }
                ForEach(folders ?? [], id: \.id) { folder in
                    Button {
                        trail.append(Crumb(id: folder.id, name: folder.name))
                    } label: {
                        pickerRow {
                            EntryIconView(icon: .folder, state: nil)
                            Text(folder.name).lineLimit(1)
                            Spacer()
                            // It opens the folder, as a row in the browser does.
                            Image(systemName: "chevron.right")
                                .font(.footnote.weight(.semibold))
                                .foregroundStyle(.tertiary)
                        }
                    }
                    .buttonStyle(.plain)
                    .fileRow()
                }
                if folders?.isEmpty == true {
                    Text("No subfolders").foregroundStyle(.secondary).fileRow()
                }
            }
            if let error {
                Text(error).foregroundStyle(.red).fileRow()
            }
        }
        .fileList()
        .navigationTitle(title)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(saving ? "Working…" : "Move here") { move(.abort) }
                    .disabled(saving || alreadyIn(entries: entries, destination: destination))
            }
        }
    }

    private func pickerRow(@ViewBuilder content: () -> some View) -> some View {
        HStack(spacing: 12, content: content)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
    }

    /// Files already at the destination; cancelling goes back to choosing another folder.
    private func conflictList(_ found: MoveResult) -> some View {
        List {
            Section {
                ForEach(found.conflicts, id: \.path) { conflict in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(conflict.name)
                        Group {
                            Text(conflict.path)
                            Text("Incoming: \(describe(conflict.incoming))")
                            Text("Existing: \(conflict.existing.map(describe) ?? "Also selected")")
                        }
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    }
                    .fileRow()
                }
            } footer: {
                Text("Override replaces the current content and keeps it as a version.")
            }
            if let error {
                Text(error).foregroundStyle(.red).fileRow()
            }
            Section {
                Button(saving ? "Working…" : "Override") { move(.override) }
                    .disabled(saving)
                    .fileRow()
                Button("Skip these files") { move(.skip) }
                    .disabled(saving)
                    .fileRow()
            }
        }
        .fileList()
        .navigationTitle(found.conflictsTitle)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") {
                    conflicts = nil
                    error = nil
                }
            }
        }
    }

    private func loadFolders() async {
        folders = nil
        do {
            let moving = Set(entries.map(\.id))
            folders = try await client.list(parentId: destination)
                .filter { $0.kind == .folder && !moving.contains($0.id) }
        } catch is CancellationError {
            // A newer load took over.
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }

    private func move(_ mode: MoveMode) {
        saving = true
        error = nil
        Task {
            do {
                let result = try await client.moveNodes(
                    ids: entries.map(\.id),
                    parentId: destination,
                    mode: mode
                )
                if result.conflicts.isEmpty {
                    onDone()
                } else {
                    conflicts = result
                    saving = false
                }
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                self.error = fileMessage(for: error)
                saving = false
            }
        }
    }
}

/// The system's picker for where to save a file; it copies the file there.
struct ImportPicker: UIViewControllerRepresentable {
    let onPick: ([URL]) -> Void

    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        // A copy, which stays readable after the picker closes and for a retry.
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item], asCopy: true)
        picker.allowsMultipleSelection = true
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ controller: UIDocumentPickerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(onPick: onPick) }

    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        let onPick: ([URL]) -> Void

        init(onPick: @escaping ([URL]) -> Void) {
            self.onPick = onPick
        }

        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            onPick(urls)
        }
    }
}

/// The system's screen for choosing where a file goes.
struct ExportPicker: UIViewControllerRepresentable {
    let url: URL
    let onFinish: () -> Void

    func makeUIViewController(context: Context) -> UIDocumentPickerViewController {
        let picker = UIDocumentPickerViewController(forExporting: [url], asCopy: true)
        picker.delegate = context.coordinator
        return picker
    }

    func updateUIViewController(_ controller: UIDocumentPickerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator {
        Coordinator(onFinish: onFinish)
    }

    final class Coordinator: NSObject, UIDocumentPickerDelegate {
        private let onFinish: () -> Void

        init(onFinish: @escaping () -> Void) {
            self.onFinish = onFinish
        }

        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            onFinish()
        }

        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
            onFinish()
        }
    }
}
