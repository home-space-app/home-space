import SwiftUI

/// Chooses the device to send to, the user's own first, then sends.
struct SendSheet: View {
    let model: SessionModel
    let payload: SendPayload
    let onSent: () -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var targets: [SendTarget]?
    @State private var chosen: SendTarget?
    @State private var showOthers = false
    @State private var sending = false
    @State private var sent = false
    @State private var error: String?

    private var own: [SendTarget] { targets?.filter(\.own) ?? [] }
    private var others: [SendTarget] { targets?.filter { !$0.own } ?? [] }
    /// Other users' devices, by user.
    private var otherUsers: [(String, [SendTarget])] {
        Dictionary(grouping: others, by: \.username).sorted { $0.key < $1.key }
    }

    private var summary: String? {
        guard case .files(let files, _) = payload else { return nil }
        let bytes = files.reduce(UInt64(0)) { total, file in
            let size = (try? FileManager.default.attributesOfItem(atPath: file.path)[.size] as? NSNumber)?
                .uint64Value ?? 0
            return total + size
        }
        return sendSummary(files: UInt32(files.count), bytes: bytes)
    }

    var body: some View {
        NavigationStack {
            List {
                if let error {
                    Text(error).foregroundStyle(.red)
                }
                if let targets {
                    if targets.isEmpty {
                        ContentUnavailableView(
                            "No Devices",
                            systemImage: "iphone.slash",
                            description: Text("Sign in to Home Space on another device to send files to it.")
                        )
                    } else {
                        Section {
                            ForEach(own, id: \.deviceId) { row($0) }
                            if own.isEmpty {
                                Text("You have no other devices.").foregroundStyle(.secondary)
                            }
                        } header: {
                            Text("Your Devices")
                        } footer: {
                            if let summary {
                                Text(summary)
                            }
                        }
                        if !others.isEmpty {
                            Section {
                                Toggle("Another User’s Device", isOn: $showOthers.animation())
                            }
                            if showOthers {
                                ForEach(otherUsers, id: \.0) { user, devices in
                                    Section(user) {
                                        ForEach(devices, id: \.deviceId) { row($0) }
                                    }
                                }
                            }
                        }
                        if let chosen {
                            Section {
                            } footer: {
                                Text(chosen.note)
                            }
                        }
                    }
                } else {
                    HStack {
                        Spacer()
                        ProgressView()
                        Spacer()
                    }
                    .listRowBackground(Color.clear)
                }
            }
            .navigationTitle(payload.count == 1 ? "Send to Device" : "Send \(payload.count) Items")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if sending {
                        ProgressView()
                    } else {
                        Button("Send") { send() }
                            .disabled(chosen == nil)
                    }
                }
            }
        }
        .interactiveDismissDisabled(sending)
        .task(id: model.devicesGeneration) { await load() }
        .onDisappear {
            // Files nobody sent go; a send that started removes them when it ends.
            if !sent, case .files(_, let folder) = payload {
                try? FileManager.default.removeItem(at: folder)
            }
        }
    }

    private func row(_ target: SendTarget) -> some View {
        Button {
            chosen = target
        } label: {
            HStack(spacing: 12) {
                Image(systemName: platformSymbol(target.platform))
                    .frame(width: 28)
                    .foregroundStyle(Color.accentColor)
                    .accessibilityLabel(platformName(target.platform))
                VStack(alignment: .leading, spacing: 2) {
                    Text(target.name).foregroundStyle(.primary)
                    Text(target.status)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if chosen?.deviceId == target.deviceId {
                    Image(systemName: "checkmark")
                        .foregroundStyle(Color.accentColor)
                        .accessibilityLabel("Selected")
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(chosen?.deviceId == target.deviceId ? .isSelected : [])
    }

    private func load() async {
        guard let center = model.transfers else { return }
        do {
            let found = try await center.targets()
            targets = found
            if chosen == nil || !found.contains(where: { $0.deviceId == chosen?.deviceId }) {
                chosen = found.first(where: \.preferred) ?? found.first(where: \.own)
            }
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

    private func send() {
        guard let center = model.transfers, let target = chosen else { return }
        switch payload {
        case .entries(let entries):
            sending = true
            Task {
                defer { sending = false }
                do {
                    try await center.send(entries: entries, to: target)
                    sent = true
                    onSent()
                    dismiss()
                    await IncomingNotices.askIfNeeded()
                } catch CoreError.Unauthorized {
                    model.sessionEnded()
                } catch {
                    self.error = fileMessage(for: error)
                }
            }
        case .files(let files, let folder):
            // Runs on with progress in Transfers.
            sent = true
            center.send(files: files, folder: folder, to: target)
            onSent()
            dismiss()
        }
    }
}
