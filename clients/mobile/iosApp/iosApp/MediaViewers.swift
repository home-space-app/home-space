import AVKit
import PDFKit
import SwiftUI

// MARK: PDF

/// A PDF, shown with PDFKit: pages one under another, with pinch zoom, text selection and the
/// share sheet.
struct PdfViewer: View {
    let model: SessionModel
    let client: CoreClient
    let source: ViewerSource
    @Environment(\.dismiss) private var dismiss
    @State private var document: PDFDocument?
    @State private var shareable: URL?
    @State private var error: String?

    var body: some View {
        NavigationStack {
            content
                .navigationTitle(source.name)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Done") { dismiss() }
                    }
                    if let shareable {
                        ToolbarItem(placement: .bottomBar) {
                            ShareLink(item: shareable)
                        }
                    }
                }
        }
        .task { await load() }
        .onDisappear {
            if let shareable {
                try? FileManager.default.removeItem(at: shareable.deletingLastPathComponent())
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if let document {
            PDFKitView(document: document)
                .ignoresSafeArea(edges: .bottom)
        } else if let error {
            ContentUnavailableView("Can't Show This File", systemImage: "doc.richtext", description: Text(error))
        } else {
            ProgressView()
        }
    }

    private func load() async {
        do {
            let data: Data
            switch source {
            case .own(let entry):
                data = try await client.readPdf(id: entry.id).bytes
            case .shared(let entry, _):
                data = try await client.readSharedPdf(file: entry)
            }
            guard let opened = PDFDocument(data: data) else {
                error = "This PDF can't be opened on this iPhone."
                return
            }
            if opened.isLocked {
                error = "This PDF is protected with a password. Download it to open it."
                return
            }
            document = opened
            shareable = try? keepForSharing(data, name: source.name, fallback: "document.pdf")
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }
}

/// PDFKit's view, which SwiftUI does not have.
private struct PDFKitView: UIViewRepresentable {
    let document: PDFDocument

    func makeUIView(context: Context) -> PDFView {
        let view = PDFView()
        view.autoScales = true
        view.displayMode = .singlePageContinuous
        view.displayDirection = .vertical
        view.document = document
        return view
    }

    func updateUIView(_ view: PDFView, context: Context) {
        if view.document !== document {
            view.document = document
        }
    }
}

// MARK: Video

/// A video, streamed from the server and played with AVKit's player.
///
/// The URL carries a stream ticket, which lasts an hour. When playback fails after a while, the
/// ticket has most likely expired: the viewer gets a new URL and resumes where it was. When it
/// fails right after starting, the phone cannot play the file.
struct VideoViewer: View {
    let model: SessionModel
    let client: CoreClient
    let source: ViewerSource
    @Environment(\.dismiss) private var dismiss
    @State private var player: AVPlayer?
    @State private var error: String?

    /// A failure this soon after a URL starts is the file, not an expired ticket.
    private static let retryAfter: TimeInterval = 60

    var body: some View {
        NavigationStack {
            content
                .background(Color.black)
                // Dark, as Photos is, so the bars read over the black around the video.
                .environment(\.colorScheme, .dark)
                .toolbarColorScheme(.dark, for: .navigationBar)
                .toolbarBackground(.visible, for: .navigationBar)
                .navigationTitle(source.name)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Done") { dismiss() }
                    }
                }
        }
        .task { await play() }
        .onDisappear { player?.pause() }
    }

    @ViewBuilder
    private var content: some View {
        if let error {
            ContentUnavailableView("Can't Play This Video", systemImage: "film", description: Text(error))
                .foregroundStyle(.white)
        } else if let player {
            VideoPlayer(player: player)
                .ignoresSafeArea(edges: .bottom)
        } else {
            ProgressView().tint(.white)
        }
    }

    private func play() async {
        // Plays with the sound on even when the phone is set to silent, as video apps do.
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
        var resumeAt: CMTime?
        while !Task.isCancelled {
            let url: URL
            do {
                guard let parsed = URL(string: try await streamURL()) else {
                    error = "The video's address is not valid."
                    return
                }
                url = parsed
            } catch CoreError.Unauthorized {
                model.sessionEnded()
                return
            } catch {
                self.error = fileMessage(for: error)
                return
            }
            let item = AVPlayerItem(url: url)
            // A format the phone does not play, such as WebM, never fails as an item: it just
            // does not start. Ask before playing.
            if resumeAt == nil, let playable = try? await item.asset.load(.isPlayable), !playable {
                error = "This iPhone can't play this video's format. Download it to open it in another app."
                return
            }
            if let player {
                player.replaceCurrentItem(with: item)
            } else {
                player = AVPlayer(playerItem: item)
            }
            if let resumeAt {
                await player?.seek(to: resumeAt)
            }
            player?.play()
            let started = Date()
            await failure(of: item)
            if Task.isCancelled {
                return
            }
            if Date().timeIntervalSince(started) < Self.retryAfter {
                error = item.error?.localizedDescription ?? "This video can't be played on this iPhone."
                player?.pause()
                return
            }
            resumeAt = player?.currentTime()
        }
    }

    private func streamURL() async throws -> String {
        switch source {
        case .own(let entry):
            try await client.streamUrl(id: entry.id)
        case .shared(let entry, _):
            try await client.sharedStreamUrl(file: entry)
        }
    }
}

/// Returns once `item` fails to load or stops partway through.
private func failure(of item: AVPlayerItem) async {
    await withTaskGroup(of: Void.self) { group in
        group.addTask {
            for await status in item.publisher(for: \.status).values where status == .failed {
                return
            }
        }
        group.addTask {
            let stopped = NotificationCenter.default.notifications(
                named: AVPlayerItem.failedToPlayToEndTimeNotification, object: item)
            for await _ in stopped {
                return
            }
        }
        await group.next()
        group.cancelAll()
    }
}

// MARK: Sharing

/// A copy of `data` named `name` in a folder of its own, for the share sheet, which needs a file
/// to offer Save to Files. Remove the folder when the viewer closes.
func keepForSharing(_ data: Data, name: String, fallback: String) throws -> URL {
    let folder = FileManager.default.temporaryDirectory
        .appendingPathComponent("viewer", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    let url = folder.appendingPathComponent(safeFileName(name, fallback: fallback))
    try data.write(to: url)
    return url
}
