import ImageIO
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// The file a built-in viewer shows: one of the user's own, or one shared with them.
enum ViewerSource {
    case own(Entry)
    /// Read-only; `siblings` are the files beside it, which the image viewer steps through.
    case shared(SharedEntry, siblings: [SharedEntry])

    var name: String {
        switch self {
        case .own(let entry): entry.name
        case .shared(let entry, _): entry.name
        }
    }
}

/// A file open in a built-in viewer.
struct OpenViewer: Identifiable {
    let id = UUID()
    let source: ViewerSource
    let info: ViewerInfo
}

/// Shows the viewer for `viewer`, full screen.
struct ViewerScreen: View {
    let model: SessionModel
    let client: CoreClient
    let viewer: OpenViewer

    var body: some View {
        switch viewer.info.kind {
        case .text:
            TextViewer(model: model, client: client, source: viewer.source, info: viewer.info)
        case .image:
            ImageViewer(model: model, client: client, source: viewer.source)
        }
    }
}

/// Registers the image types this phone decodes beyond the ones every client shows, such as HEIC.
func registerPlatformImageTypes() {
    let decodable = (CGImageSourceCopyTypeIdentifiers() as? [String]) ?? []
    let mimes = Set(decodable.compactMap { UTType($0)?.preferredMIMEType })
    let wanted = ["image/heic", "image/heif", "image/avif", "image/webp"].filter(mimes.contains)
    if !wanted.isEmpty {
        addViewerTypes(kind: .image, types: wanted)
    }
}

// MARK: Text

/// A text file, with Edit for the user's own files when they are UTF-8. Saving keeps the old
/// content as a version.
private struct TextViewer: View {
    let model: SessionModel
    let client: CoreClient
    let source: ViewerSource
    let info: ViewerInfo
    @Environment(\.dismiss) private var dismiss
    @State private var file: TextFile?
    @State private var text: String?
    @State private var draft = ""
    @State private var editing = false
    @State private var saving = false
    @State private var error: String?
    @State private var askingDiscard = false

    private var dirty: Bool { editing && draft != (text ?? "") }

    private var canEdit: Bool {
        guard case .own = source, info.editable, let file else { return false }
        return file.editable
    }

    var body: some View {
        NavigationStack {
            content
                .navigationTitle(file?.entry.name ?? source.name)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { toolbar }
        }
        .interactiveDismissDisabled(dirty)
        .confirmationDialog("Discard your changes?", isPresented: $askingDiscard, titleVisibility: .visible) {
            Button("Discard Changes", role: .destructive) {
                editing = false
                dismiss()
            }
            Button("Keep Editing", role: .cancel) {}
        }
        .task { await load() }
    }

    @ViewBuilder
    private var content: some View {
        if editing {
            TextEditor(text: $draft)
                .font(.body.monospaced())
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .padding(.horizontal, 12)
                .disabled(saving)
        } else if let text {
            ScrollView {
                Text(text.isEmpty ? " " : text)
                    .font(.body.monospaced())
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
            }
        } else if let error {
            ContentUnavailableView("Can't Show This File", systemImage: "doc.text", description: Text(error))
        } else {
            ProgressView()
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .cancellationAction) {
            if editing {
                Button("Cancel") {
                    if dirty {
                        askingDiscard = true
                    } else {
                        editing = false
                    }
                }
            } else {
                Button("Done") { dismiss() }
            }
        }
        ToolbarItem(placement: .confirmationAction) {
            if editing {
                if saving {
                    ProgressView()
                } else {
                    Button("Save") { Task { await save() } }
                        .disabled(!dirty)
                }
            } else if canEdit {
                Button("Edit") {
                    draft = text ?? ""
                    editing = true
                }
            }
        }
        if !editing, let text {
            ToolbarItem(placement: .bottomBar) {
                ShareLink(item: text, preview: SharePreview(source.name))
            }
        }
    }

    private func load() async {
        do {
            switch source {
            case .own(let entry):
                let loaded = try await client.readText(id: entry.id)
                file = loaded
                text = loaded.text
            case .shared(let entry, _):
                text = try await client.readSharedText(file: entry)
            }
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }

    private func save() async {
        guard let file else { return }
        saving = true
        defer { saving = false }
        do {
            let entry = try await client.saveText(id: file.entry.id, text: draft, format: file.format)
            self.file = TextFile(entry: entry, text: draft, editable: true, format: file.format)
            text = draft
            editing = false
            model.reload()
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            model.notice = "Could not save “\(file.entry.name)”: \(fileMessage(for: error))"
        }
    }
}

// MARK: Images

/// One picture of the gallery, with how to read it.
private struct ImagePage: Identifiable, Equatable {
    let id: String
    let name: String
    let load: @Sendable () async throws -> Data

    static func == (a: ImagePage, b: ImagePage) -> Bool { a.id == b.id }
}

/// The pictures of a folder, swiped through, each one zoomable.
private struct ImageViewer: View {
    let model: SessionModel
    let client: CoreClient
    let source: ViewerSource
    @Environment(\.dismiss) private var dismiss
    @State private var pages: [ImagePage] = []
    @State private var selection: String?
    @State private var shareable: [String: URL] = [:]

    private var current: ImagePage? { pages.first { $0.id == selection } }

    var body: some View {
        NavigationStack {
            TabView(selection: $selection) {
                ForEach(pages) { page in
                    ImagePageView(model: model, page: page) { url in shareable[page.id] = url }
                        .tag(Optional(page.id))
                }
            }
            .tabViewStyle(.page(indexDisplayMode: .never))
            .background(Color.black)
            // Dark, as Photos is, so the bars read over the black around a picture.
            .environment(\.colorScheme, .dark)
            .toolbarColorScheme(.dark, for: .navigationBar, .bottomBar)
            .navigationTitle(current?.name ?? source.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(.visible, for: .navigationBar, .bottomBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                }
                if pages.count > 1, let index = pages.firstIndex(where: { $0.id == selection }) {
                    ToolbarItem(placement: .principal) {
                        VStack(spacing: 0) {
                            Text(current?.name ?? "").font(.headline).lineLimit(1)
                            Text("\(index + 1) of \(pages.count)")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                if let id = selection, let url = shareable[id] {
                    ToolbarItem(placement: .bottomBar) {
                        ShareLink(item: url)
                    }
                }
            }
        }
        .task { await loadGallery() }
        .onDisappear {
            for url in shareable.values {
                try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
            }
        }
    }

    private func loadGallery() async {
        let client = client
        switch source {
        case .own(let entry):
            let gallery = try? await client.gallery(id: entry.id)
            var entries = gallery?.entries ?? []
            if gallery?.index == nil {
                entries = [entry]
            }
            pages = entries.map { item in
                ImagePage(id: item.id, name: item.name) { try await client.readImage(id: item.id).bytes }
            }
            selection = entry.id
        case .shared(let entry, let siblings):
            var pictures = siblings.filter {
                $0.kind == .file && viewerFor(mime: $0.mime, size: $0.size)?.kind == .image
            }
            if !pictures.contains(where: { $0.id == entry.id }) {
                pictures = [entry]
            }
            pages = pictures.map { item in
                ImagePage(id: item.id, name: item.name) { try await client.readSharedImage(file: item) }
            }
            selection = entry.id
        }
    }
}

/// One picture, read when its page comes up and scaled down to what the screen can show.
private struct ImagePageView: View {
    let model: SessionModel
    let page: ImagePage
    let onShareable: (URL) -> Void
    @State private var image: UIImage?
    @State private var error: String?

    var body: some View {
        Group {
            if let image {
                ZoomableImage(image: image)
                    .accessibilityLabel(page.name)
            } else if let error {
                ContentUnavailableView("Can't Show This Picture", systemImage: "photo", description: Text(error))
                    .foregroundStyle(.white)
            } else {
                ProgressView().tint(.white)
            }
        }
        .task {
            do {
                let data = try await page.load()
                let screen = UIScreen.main
                let side = max(screen.bounds.width, screen.bounds.height) * screen.scale * 2
                image = await Task.detached { downsample(data, maxPixels: side) }.value
                if image == nil {
                    error = "This picture can't be decoded on this iPhone."
                } else if let url = try? keep(data) {
                    onShareable(url)
                }
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                self.error = fileMessage(for: error)
            }
        }
    }

    /// A copy of the picture for the share sheet, which needs a file to offer Save to Files.
    private func keep(_ data: Data) throws -> URL {
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("viewer", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appendingPathComponent(safeFileName(page.name, fallback: page.id))
        try data.write(to: url)
        return url
    }
}

/// Decodes a picture no larger than `maxPixels` on its long side, so a big photo doesn't take
/// hundreds of megabytes.
private func downsample(_ data: Data, maxPixels: CGFloat) -> UIImage? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
    let options: [CFString: Any] = [
        kCGImageSourceCreateThumbnailFromImageAlways: true,
        kCGImageSourceCreateThumbnailWithTransform: true,
        kCGImageSourceShouldCacheImmediately: true,
        kCGImageSourceThumbnailMaxPixelSize: maxPixels,
    ]
    guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
    return UIImage(cgImage: image)
}

/// A picture fitted to the screen, with pinch and double-tap zoom. SwiftUI has no zooming
/// scroll view before iOS 18, so this is UIKit's.
private struct ZoomableImage: UIViewRepresentable {
    let image: UIImage

    func makeUIView(context: Context) -> ZoomingView { ZoomingView(image: image) }

    func updateUIView(_ view: ZoomingView, context: Context) {
        view.show(image)
    }

    final class ZoomingView: UIScrollView, UIScrollViewDelegate {
        private let imageView = UIImageView()

        init(image: UIImage) {
            super.init(frame: .zero)
            delegate = self
            minimumZoomScale = 1
            maximumZoomScale = 5
            showsVerticalScrollIndicator = false
            showsHorizontalScrollIndicator = false
            contentInsetAdjustmentBehavior = .never
            imageView.contentMode = .scaleAspectFit
            imageView.image = image
            addSubview(imageView)
            let doubleTap = UITapGestureRecognizer(target: self, action: #selector(toggleZoom(_:)))
            doubleTap.numberOfTapsRequired = 2
            addGestureRecognizer(doubleTap)
        }

        required init?(coder: NSCoder) { nil }

        func show(_ image: UIImage) {
            guard imageView.image !== image else { return }
            imageView.image = image
            setZoomScale(1, animated: false)
            setNeedsLayout()
        }

        override func layoutSubviews() {
            super.layoutSubviews()
            if zoomScale == 1 {
                imageView.frame = bounds
                contentSize = bounds.size
            }
        }

        func viewForZooming(in scrollView: UIScrollView) -> UIView? { imageView }

        @objc private func toggleZoom(_ tap: UITapGestureRecognizer) {
            if zoomScale > 1 {
                setZoomScale(1, animated: true)
            } else {
                let point = tap.location(in: imageView)
                let size = CGSize(width: bounds.width / 2.5, height: bounds.height / 2.5)
                zoom(to: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2,
                                width: size.width, height: size.height), animated: true)
            }
        }
    }
}
