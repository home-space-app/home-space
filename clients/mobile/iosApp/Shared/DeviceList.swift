import SwiftUI

/// The devices files can be sent to, the user's own first; tapping one chooses it. Shared by the
/// app's Send sheet and the share extension.
struct DeviceList: View {
    /// `nil` while loading.
    let targets: [SendTarget]?
    @Binding var chosen: SendTarget?
    @Binding var showOthers: Bool
    var error: String?
    /// What is being sent, under the user's own devices.
    var summary: String?

    private var own: [SendTarget] { targets?.filter(\.own) ?? [] }
    private var others: [SendTarget] { targets?.filter { !$0.own } ?? [] }
    /// Other users' devices, by user.
    private var otherUsers: [(String, [SendTarget])] {
        Dictionary(grouping: others, by: \.username).sorted { $0.key < $1.key }
    }

    /// The device to offer first among `found`: the one already chosen if it is still there.
    static func choice(in found: [SendTarget], keeping current: SendTarget?) -> SendTarget? {
        if let current, let same = found.first(where: { $0.deviceId == current.deviceId }) {
            return same
        }
        return found.first(where: \.preferred) ?? found.first(where: \.own)
    }

    var body: some View {
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
}
