import SwiftUI

/// The "New version available" choices kept on this iPhone, never synced.
@MainActor
enum UpdatePrefs {
    private static let checkKey = "update.check"
    private static let skippedKey = "update.skippedVersion"
    private static let bannerKey = "update.bannerDismissedForServer"

    private static var defaults: UserDefaults { SessionStore.defaults }

    /// Whether to ask GitHub for the latest release when the app starts; on unless turned off.
    static var checks: Bool {
        get { defaults.object(forKey: checkKey) as? Bool ?? true }
        set {
            defaults.set(newValue, forKey: checkKey)
            if !newValue { UpdateCheck.shared.offer = nil }
        }
    }

    static var skipped: String? { defaults.string(forKey: skippedKey) }

    /// Skips `version`: only a newer release is offered from now on.
    static func skip(_ version: String) {
        defaults.set(version, forKey: skippedKey)
        UpdateCheck.shared.offer = nil
    }

    /// Whether the red banner shows for `check`: until dismissed for this server version.
    static func showsBanner(for check: VersionCheck?) -> Bool {
        guard let check, check.banner != nil else { return false }
        return defaults.string(forKey: bannerKey) != (check.serverVersion ?? "")
    }

    static func dismissBanner(for check: VersionCheck) {
        defaults.set(check.serverVersion ?? "", forKey: bannerKey)
    }
}

/// The release the toast offers, read once each time the app starts.
@MainActor
@Observable
final class UpdateCheck {
    static let shared = UpdateCheck()

    var offer: UpdateOffer?
    private var checked = false

    /// Asks for the latest release unless already asked or turned off. A failure shows nothing.
    func run() async {
        guard !checked, UpdatePrefs.checks else { return }
        checked = true
        offer = await newVersion(skipped: UpdatePrefs.skipped, releasesUrl: nil)
    }
}

extension VersionTone {
    var color: Color {
        switch self {
        case .same: .green
        case .warning: .orange
        case .error: .red
        }
    }
}

/// A dot in the colour of a version state.
struct VersionDot: View {
    let tone: VersionTone
    var size: CGFloat = 8

    var body: some View {
        Circle()
            .fill(tone.color)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

/// The Version row of the profile; opens [VersionSheet].
struct VersionRow: View {
    let check: VersionCheck
    @State private var showing = false

    var body: some View {
        Button {
            showing = true
        } label: {
            HStack(spacing: 8) {
                VersionDot(tone: check.tone)
                Text("This app \(check.appVersion)")
                    .foregroundStyle(.primary)
                Spacer()
                Text("Server \(check.serverVersion ?? "unknown")")
                    .foregroundStyle(.secondary)
                Image(systemName: "chevron.right")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(.tertiary)
            }
        }
        .accessibilityLabel("\(check.title). \(check.chip)")
        .sheet(isPresented: $showing) { VersionSheet(check: check) }
    }
}

/// Both versions, the state and what to do about it.
struct VersionSheet: View {
    let check: VersionCheck
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack(spacing: 8) {
                        VersionDot(tone: check.tone, size: 10)
                        Text(check.title)
                            .font(.headline)
                            .foregroundStyle(check.tone.color)
                    }
                    LabeledContent("This App", value: check.appVersion)
                    LabeledContent("Server", value: check.serverVersion ?? "Unknown")
                } footer: {
                    Text(check.hint)
                }
                if !check.missingFeatures.isEmpty {
                    Section("Won't Work Until the Server Is Updated") {
                        ForEach(check.missingFeatures, id: \.self) { Text($0) }
                    }
                }
            }
            .navigationTitle("Version")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

/// The red banner over Files, until dismissed for this server version.
struct VersionBanner: View {
    let check: VersionCheck
    let onDismiss: () -> Void
    @State private var showing = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.octagon.fill")
                .foregroundStyle(.red)
            VStack(alignment: .leading, spacing: 2) {
                Text(check.title).font(.subheadline.weight(.semibold))
                Text(check.banner ?? "").font(.caption)
                Button("Details") { showing = true }
                    .font(.caption.weight(.semibold))
            }
            Spacer()
            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .foregroundStyle(.secondary)
            }
            .accessibilityLabel("Dismiss")
        }
        .padding(12)
        .background(.red.opacity(0.12), in: RoundedRectangle(cornerRadius: 12))
        .padding(.horizontal)
        .sheet(isPresented: $showing) { VersionSheet(check: check) }
    }
}

/// The "New version available" card above the tab bar.
struct UpdateCard: View {
    let offer: UpdateOffer
    @Environment(\.openURL) private var openURL
    private let labels = updateLabels()

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Image(systemName: "sparkles")
                    .foregroundStyle(Color.accentColor)
                Text(labels.title).font(.subheadline.weight(.semibold))
                Spacer()
                Button {
                    UpdateCheck.shared.offer = nil
                } label: {
                    Image(systemName: "xmark")
                        .foregroundStyle(.secondary)
                }
                .accessibilityLabel("Close")
            }
            Text(offer.message).font(.footnote)
            Text(labels.iosNote).font(.footnote).foregroundStyle(.secondary)
            HStack(spacing: 20) {
                Spacer()
                Button(labels.skip) { UpdatePrefs.skip(offer.version) }
                Button(labels.openWeb) {
                    if let url = URL(string: offer.url) { openURL(url) }
                }
                .fontWeight(.semibold)
            }
            .font(.subheadline)
            .padding(.top, 4)
        }
        .padding(14)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14))
        .shadow(color: .black.opacity(0.15), radius: 10, y: 4)
        .padding(.horizontal)
    }
}
