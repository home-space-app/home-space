import QuickLook
import SwiftUI

/// The signed-in app: Files, Transfers, Shared and Settings, as on Android. What can pop up over
/// any tab is attached here.
struct MainTabs: View {
    let model: SessionModel
    let client: CoreClient
    let user: SignedInUser
    @Bindable private var router = AppRouter.shared

    /// What needs the user: failed uploads, and transfers waiting for them, as on Android.
    private var badge: Int {
        model.uploads.count { $0.state == .failed } + (model.transfers?.waiting ?? 0)
    }

    var body: some View {
        TabView(selection: $router.tab) {
            BrowseView(model: model, client: client, user: user)
                .tabItem { Label("Files", systemImage: "folder") }
                .tag(AppTab.files)
            NavigationStack {
                TransfersView(model: model, client: client)
            }
            .tabItem { Label("Transfers", systemImage: "arrow.up.arrow.down") }
            .badge(badge)
            .tag(AppTab.transfers)
            NavigationStack {
                SharedView(model: model, client: client)
            }
            .tabItem { Label("Shared", systemImage: "person.2") }
            .tag(AppTab.shared)
            NavigationStack {
                SettingsView(model: model, client: client, user: user)
            }
            .tabItem { Label("Settings", systemImage: "gearshape") }
            .tag(AppTab.settings)
        }
        .background(
            QuickLookPresenter(
                url: Binding(get: { model.preview }, set: { model.preview = $0 }),
                editable: model.previewEditable,
                onChange: { model.previewEnded() }
            )
        )
        .fullScreenCover(item: Binding(get: { model.viewer }, set: { model.viewer = $0 })) { viewer in
            ViewerScreen(model: model, client: client, viewer: viewer)
        }
        .alert(
            "Remove “\(model.removalWarning?.entry.name ?? "")” from this iPhone?",
            isPresented: Binding(
                get: { model.removalWarning != nil },
                set: { if !$0 { model.removalWarning = nil } }
            ),
            presenting: model.removalWarning
        ) { warning in
            Button("Remove", role: .destructive) {
                model.removeOfflineCopy(client: client, entry: warning.entry, confirmed: true)
            }
            Button("Cancel", role: .cancel) {}
        } message: { warning in
            Text(unsentEditsMessage(warning.names))
        }
        .alert(
            "Sign out?",
            isPresented: Binding(
                get: { model.signOutWarning != nil },
                set: { if !$0 { model.signOutWarning = nil } }
            ),
            presenting: model.signOutWarning
        ) { _ in
            Button("Sign Out", role: .destructive) { model.signOut() }
            Button("Cancel", role: .cancel) {}
        } message: { names in
            Text(unsentEditsMessage(names))
        }
        .sheet(
            item: Binding(
                get: { model.pendingExport },
                set: { if $0 == nil { model.exportFinished() } }
            )
        ) { export in
            ExportPicker(url: export.url) { model.exportFinished() }
                .ignoresSafeArea()
        }
        .overlay(alignment: .bottom) {
            if let offer = UpdateCheck.shared.offer {
                // Above the tab bar.
                UpdateCard(offer: offer)
                    .padding(.bottom, 60)
            }
        }
        .task { await UpdateCheck.shared.run() }
        .modifier(TransferDialogs(center: model.transfers))
        .alert(
            "Something went wrong",
            isPresented: Binding(
                get: { model.notice != nil },
                set: { if !$0 { model.notice = nil } }
            ),
            actions: { Button("OK") {} },
            message: { Text(model.notice ?? "") }
        )
    }
}

/// What send to device asks or says, wherever the user is.
private struct TransferDialogs: ViewModifier {
    let center: TransferCenter?
    @Environment(\.openURL) private var openURL

    func body(content: Content) -> some View {
        content
            .alert(
                linkTitle,
                isPresented: Binding(
                    get: { center?.pendingLink != nil },
                    set: { if !$0 { center?.pendingLink = nil } }
                ),
                presenting: center?.pendingLink
            ) { pending in
                Button("Open") { center?.openConfirmed(pending) }
                Button("Cancel", role: .cancel) {}
            } message: { pending in
                Text(linkMessage(pending.link))
            }
            .alert(
                "Saved to Files",
                isPresented: Binding(
                    get: { center?.saved != nil },
                    set: { if !$0 { center?.saved = nil } }
                ),
                presenting: center?.saved
            ) { saved in
                Button("Show in Files") {
                    if let url = filesURL(saved.url) {
                        openURL(url)
                    }
                }
                Button("OK", role: .cancel) {}
            } message: { saved in
                Text("“\(saved.title)” is in On My iPhone › Home Space › Received.")
            }
            .alert(
                "Something went wrong",
                isPresented: Binding(
                    get: { center?.notice != nil },
                    set: { if !$0 { center?.notice = nil } }
                ),
                actions: { Button("OK") {} },
                message: { Text(center?.notice ?? "") }
            )
    }

    private var linkTitle: String {
        guard let link = center?.pendingLink?.link else { return "Open Link?" }
        return link.web ? "Open \(link.host ?? "this link")?" : "Open in another app?"
    }

    private func linkMessage(_ link: TransferLink) -> String {
        let more = link.others == 0 ? "" : "\n\n\(link.others) more \(link.others == 1 ? "link isn't" : "links aren't") opened."
        return link.uri + more
    }

    /// The Files app at `url`; it opens the folder that holds a file.
    private func filesURL(_ url: URL) -> URL? {
        var parts = URLComponents(url: url, resolvingAgainstBaseURL: false)
        parts?.scheme = "shareddocuments"
        return parts?.url
    }
}

func unsentEditsMessage(_ names: [String]) -> String {
    let shown = names.prefix(3).joined(separator: ", ")
    let more = names.count > 3 ? " and \(names.count - 3) more" : ""
    return "Edits to \(shown)\(more) aren't on the server yet and will be lost."
}

/// Shows a file in the system's Quick Look, whose markup tools save into the file when it is
/// `editable`. The sync then sends the change to the server. Presented by UIKit, because SwiftUI's
/// `quickLookPreview` offers no way to allow editing.
private struct QuickLookPresenter: UIViewControllerRepresentable {
    @Binding var url: URL?
    let editable: Bool
    let onChange: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIViewController(context: Context) -> UIViewController { UIViewController() }

    func updateUIViewController(_ host: UIViewController, context: Context) {
        let coordinator = context.coordinator
        coordinator.onDismiss = {
            url = nil
            onChange()
        }
        coordinator.onChange = onChange
        coordinator.editable = editable
        guard let url else {
            coordinator.preview?.dismiss(animated: true)
            coordinator.preview = nil
            return
        }
        guard coordinator.url != url || coordinator.preview == nil else { return }
        coordinator.url = url
        let preview = QLPreviewController()
        preview.dataSource = coordinator
        preview.delegate = coordinator
        coordinator.preview = preview
        // Not while SwiftUI is updating the view.
        DispatchQueue.main.async { host.present(preview, animated: true) }
    }

    final class Coordinator: NSObject, QLPreviewControllerDataSource, QLPreviewControllerDelegate {
        var url: URL?
        var editable = true
        var preview: QLPreviewController?
        var onDismiss: () -> Void = {}
        var onChange: () -> Void = {}

        func numberOfPreviewItems(in controller: QLPreviewController) -> Int { url == nil ? 0 : 1 }

        func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> QLPreviewItem {
            (url ?? URL(fileURLWithPath: "/")) as NSURL
        }

        func previewController(
            _ controller: QLPreviewController,
            editingModeFor previewItem: QLPreviewItem
        ) -> QLPreviewItemEditingMode {
            editable ? .updateContents : .disabled
        }

        func previewController(_ controller: QLPreviewController, didUpdateContentsOf previewItem: QLPreviewItem) {
            onChange()
        }

        func previewControllerDidDismiss(_ controller: QLPreviewController) {
            preview = nil
            url = nil
            onDismiss()
        }
    }
}
