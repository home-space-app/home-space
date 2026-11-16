import SwiftUI

/// The height of a row, measured from the Files app on an iPhone. The Android client uses the same.
let rowHeight: CGFloat = 64

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
