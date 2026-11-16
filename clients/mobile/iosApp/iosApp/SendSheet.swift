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
            DeviceList(
                targets: targets, chosen: $chosen, showOthers: $showOthers, error: error, summary: summary
            )
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

    private func load() async {
        guard let center = model.transfers else { return }
        do {
            let found = try await center.targets()
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
