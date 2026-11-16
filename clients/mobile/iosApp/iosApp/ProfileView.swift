import SwiftUI

/// Who is signed in, to which server, and the user's devices.
struct ProfileView: View {
    let model: SessionModel
    let client: CoreClient
    let user: SignedInUser
    @State private var devices: [UserDevice]?
    @State private var error: String?

    var body: some View {
        List {
            Section("Account") {
                LabeledContent("Username", value: user.username)
                LabeledContent("Server", value: client.baseUrl())
            }
            Section {
                if let error {
                    Text(error).foregroundStyle(.red)
                }
                if let devices {
                    ForEach(devices, id: \.id) { DeviceRow(device: $0) }
                } else if error == nil {
                    ProgressView()
                }
            } header: {
                Text("Devices")
            } footer: {
                Text("Devices that are signed in to your account, the most recently seen first.")
            }
            Section {
                Button("Sign Out", role: .destructive) { model.requestSignOut(client: client) }
            }
        }
        .navigationTitle("Profile")
        .refreshable { await load() }
        .task(id: model.devicesGeneration) { await load() }
    }

    private func load() async {
        do {
            devices = try await client.devices()
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

private struct DeviceRow: View {
    let device: UserDevice

    private var status: String {
        if device.current {
            return "This iPhone"
        }
        if !device.signedIn {
            return "Signed out"
        }
        if device.online {
            return "Online"
        }
        return "Last seen \(device.lastSeenAt.formatted(.relative(presentation: .named)))"
    }

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: platformSymbol(device.platform))
                .frame(width: 28)
                .foregroundStyle(device.signedIn ? Color.accentColor : Color.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(device.name)
                Text("\(platformName(device.platform)) · \(status)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if device.online {
                Circle()
                    .fill(.green)
                    .frame(width: 8, height: 8)
                    .accessibilityHidden(true)
            }
        }
        .opacity(device.signedIn ? 1 : 0.5)
        .accessibilityElement(children: .combine)
    }
}
