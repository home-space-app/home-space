import SwiftUI

@main
struct iOSApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model = SessionModel()
    @Environment(\.scenePhase) private var scenePhase

    init() {
        BackgroundSync.register()
        registerPlatformImageTypes()
    }

    var body: some Scene {
        WindowGroup {
            RootView(model: model)
                .onChange(of: scenePhase, initial: true) { _, phase in
                    model.appInFront(phase == .active)
                }
        }
    }
}

/// Picks the screen for the session, which the model tries to resume once at launch.
private struct RootView: View {
    let model: SessionModel

    var body: some View {
        Group {
            switch model.session {
            case .resuming(let server, let error):
                ResumingView(model: model, server: server, error: error)
            case .signedOut:
                LoginView(model: model)
            case .signedIn(let client, let user):
                MainTabs(model: model, client: client, user: user)
            }
        }
        .task { model.start() }
    }
}
