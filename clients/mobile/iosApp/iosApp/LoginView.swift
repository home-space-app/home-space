import AuthenticationServices
import SwiftUI

/// Shown while a saved session is being tried, and when trying it failed.
struct ResumingView: View {
    let model: SessionModel
    let server: String
    let error: String?

    var body: some View {
        VStack(spacing: 16) {
            if let error {
                Text("Could not resume your session").font(.headline)
                Text(error).foregroundStyle(.red)
                Button("Retry") { model.retryResume() }
                    .buttonStyle(.borderedProminent)
                Button("Sign out") { model.signOut() }
            } else {
                ProgressView()
                Text("Resuming your session…")
                Text(server).font(.caption).foregroundStyle(.secondary)
            }
        }
        .multilineTextAlignment(.center)
        .padding()
    }
}

struct LoginView: View {
    @Bindable var model: SessionModel
    @Environment(\.webAuthenticationSession) private var webAuthenticationSession

    /// The scheme of `protocol::paths::MOBILE_OIDC_REDIRECT`.
    private static let callbackScheme = "app.homespace"

    var body: some View {
        NavigationStack {
            Form {
                Section("Server") {
                    TextField(
                        "Server",
                        text: Binding(get: { model.server }, set: { model.serverChanged(to: $0) })
                    )
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .keyboardType(.URL)
                }
                switch model.probe {
                case .checking:
                    Section { Text("Contacting the server…") }
                case .failed(let message):
                    Section {
                        Text(message).foregroundStyle(.red)
                        Button("Retry") { model.probeNow() }
                    }
                case .ready(let options):
                    signIn(options)
                }
                if let error = model.error {
                    Section { Text(error).foregroundStyle(.red) }
                }
            }
            .disabled(model.busy)
            .navigationTitle("Home Space")
        }
    }

    /// A server with the web page signs in there, whatever the account needs; the form is for
    /// older servers.
    @ViewBuilder
    private func signIn(_ options: SignInOptions) -> some View {
        if options.browser {
            Section {
                Button {
                    Task { await model.signInOnWebPage(authenticate: authenticate) }
                } label: {
                    Text(model.busy ? "Signing in…" : "Sign in")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .listRowInsets(EdgeInsets())
                .listRowBackground(Color.clear)
            }
        } else {
            nativeSignIn(options)
        }
    }

    @ViewBuilder
    private func nativeSignIn(_ options: SignInOptions) -> some View {
        if let name = options.ssoName {
            Section {
                Button("Sign in with \(name)") {
                    Task { await model.signInWithSso(authenticate: authenticate) }
                }
            } footer: {
                if options.password {
                    Text("or")
                        .frame(maxWidth: .infinity)
                }
            }
        }
        if options.password {
            Section("Sign in to your files") {
                TextField("Username", text: $model.username)
                    .textContentType(.username)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                SecureField("Password", text: $model.password)
                    .textContentType(.password)
                    .onSubmit { submit() }
                Button(model.busy ? "Signing in…" : "Sign in") { submit() }
                    .disabled(!canSubmit)
            }
        }
    }

    private var canSubmit: Bool {
        !model.username.trimmingCharacters(in: .whitespaces).isEmpty && !model.password.isEmpty
    }

    private func submit() {
        if canSubmit {
            model.signInWithPassword()
        }
    }

    /// Shows the browser sheet; closing it is not a failure, so it comes back as `nil`.
    private func authenticate(_ url: URL) async throws -> URL? {
        do {
            return try await webAuthenticationSession.authenticate(
                using: url,
                callbackURLScheme: Self.callbackScheme
            )
        } catch ASWebAuthenticationSessionError.canceledLogin {
            return nil
        }
    }
}

#Preview {
    LoginView(model: SessionModel())
}
