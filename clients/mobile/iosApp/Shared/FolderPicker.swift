import SwiftUI

/// A folder opened on the way down from Home.
struct FolderCrumb: Equatable, Hashable {
    let id: String
    let name: String
}

/// Where a path of opened folders leads, as the folder pickers say it.
func folderLocation(_ trail: [FolderCrumb]) -> String {
    trail.isEmpty ? "Home" : "/" + trail.map(\.name).joined(separator: "/")
}

/// The folders of the folder at the end of `trail`, to open one or go back up. Shared by Move in
/// the app and Save in the share extension; the caller loads `folders` for the last folder.
struct FolderPicker: View {
    let heading: String
    @Binding var trail: [FolderCrumb]
    /// `nil` while loading.
    let folders: [Entry]?
    var error: String?

    var body: some View {
        List {
            Section(heading) {
                if !trail.isEmpty {
                    Button { trail.removeLast() } label: {
                        row {
                            Image(systemName: "arrow.up").frame(width: 26, height: 26)
                            Text("Up")
                        }
                    }
                    .buttonStyle(.plain)
                    .fileRow()
                }
                ForEach(folders ?? [], id: \.id) { folder in
                    Button {
                        trail.append(FolderCrumb(id: folder.id, name: folder.name))
                    } label: {
                        row {
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
                if folders == nil {
                    ProgressView().frame(maxWidth: .infinity).fileRow()
                } else if folders?.isEmpty == true {
                    Text("No subfolders").foregroundStyle(.secondary).fileRow()
                }
            }
            if let error {
                Text(error).foregroundStyle(.red).fileRow()
            }
        }
        .fileList()
    }

    private func row(@ViewBuilder content: () -> some View) -> some View {
        HStack(spacing: 12, content: content)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
    }
}
