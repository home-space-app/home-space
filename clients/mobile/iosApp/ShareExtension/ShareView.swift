import SwiftUI

private enum ShareStep: Hashable {
    case save
    case send
}

/// The share sheet: what was shared, then Save to a folder or Send to a device.
struct ShareView: View {
    @Bindable var model: ShareModel
    @State private var path: [ShareStep] = []

    var body: some View {
        NavigationStack(path: $path) {
            content
                .navigationTitle("Home Space")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { model.cancel() }
                    }
                }
                .navigationDestination(for: ShareStep.self) { step in
                    switch step {
                    case .save: SaveView(model: model)
                    case .send: SendView(model: model)
                    }
                }
        }
        .task { await model.start() }
        .onChange(of: isSignedOut) {
            if isSignedOut { path = [] }
        }
    }

    private var isSignedOut: Bool {
        if case .signedOut = model.state { return true }
        return false
    }

    @ViewBuilder
    private var content: some View {
        switch model.state {
        case .loading:
            ProgressView()
        case .signedOut:
            ContentUnavailableView(
                "Sign In to Home Space",
                systemImage: "person.crop.circle.badge.exclamationmark",
                description: Text("Open Home Space and sign in, then share again.")
            )
        case .failed(let message):
            ContentUnavailableView {
                Label("Can’t Share", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                Button("Try Again") { Task { await model.start() } }
            }
        case .ready(_, let shared):
            List {
                Section {
                    Label {
                        Text(shared.summary).lineLimit(2)
                    } icon: {
                        Image(systemName: shared.symbol).foregroundStyle(.secondary)
                    }
                }
                Section {
                    NavigationLink(value: ShareStep.save) {
                        choice("Save to Home Space", detail: "Pick a folder", symbol: "folder")
                    }
                    NavigationLink(value: ShareStep.send) {
                        choice("Send to Device", detail: "To one of your devices, or another user’s", symbol: "paperplane")
                    }
                }
            }
        case .working(let title, let fraction):
            WorkingView(title: title, fraction: fraction)
        }
    }

    private func choice(_ title: String, detail: String, symbol: String) -> some View {
        Label {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                Text(detail).font(.caption).foregroundStyle(.secondary)
            }
        } icon: {
            Image(systemName: symbol)
        }
    }
}

private struct WorkingView: View {
    let title: String
    let fraction: Double

    var body: some View {
        VStack(spacing: 12) {
            ProgressView(value: fraction)
            Text(title).font(.subheadline).foregroundStyle(.secondary)
        }
        .padding(32)
        .frame(maxHeight: .infinity)
        .accessibilityElement(children: .combine)
    }
}

/// Picks the folder the shared files go into.
private struct SaveView: View {
    let model: ShareModel
    @State private var trail: [FolderCrumb] = []
    @State private var folders: [Entry]?
    @State private var error: String?

    var body: some View {
        Group {
            if case .working(let title, let fraction) = model.state {
                WorkingView(title: title, fraction: fraction)
            } else {
                FolderPicker(
                    heading: "To \(folderLocation(trail))", trail: $trail, folders: folders,
                    error: model.problem ?? error)
            }
        }
        .navigationTitle("Save to Home Space")
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden(isWorking)
        .toolbar {
            if isWorking {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { model.cancel() }
                }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Save") { Task { await model.save(to: trail.last) } }
                    .disabled(isWorking)
            }
        }
        .task(id: trail.last?.id) { await load() }
    }

    private var isWorking: Bool {
        if case .working = model.state { return true }
        return false
    }

    private func load() async {
        folders = nil
        do {
            folders = try await model.folders(in: trail.last?.id)
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

/// Picks the device the shared files go to.
private struct SendView: View {
    let model: ShareModel
    @State private var targets: [SendTarget]?
    @State private var chosen: SendTarget?
    @State private var showOthers = false
    @State private var error: String?

    var body: some View {
        Group {
            if case .working(let title, let fraction) = model.state {
                WorkingView(title: title, fraction: fraction)
            } else {
                DeviceList(
                    targets: targets, chosen: $chosen, showOthers: $showOthers,
                    error: model.problem ?? error, summary: summary)
            }
        }
        .navigationTitle("Send to Device")
        .navigationBarTitleDisplayMode(.inline)
        .navigationBarBackButtonHidden(isWorking)
        .toolbar {
            if isWorking {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { model.cancel() }
                }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Send") {
                    guard let chosen else { return }
                    Task { await model.send(to: chosen) }
                }
                .disabled(chosen == nil || isWorking)
            }
        }
        .task { await load() }
    }

    private var summary: String? {
        guard case .ready(_, let shared) = model.state else { return nil }
        return sendSummary(files: UInt32(shared.files.count), bytes: shared.bytes)
    }

    private var isWorking: Bool {
        if case .working = model.state { return true }
        return false
    }

    private func load() async {
        do {
            let found = try await model.targets()
            targets = found
            chosen = DeviceList.choice(in: found, keeping: chosen)
            if let chosen, !chosen.own {
                showOthers = true
            }
            error = nil
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }
}
