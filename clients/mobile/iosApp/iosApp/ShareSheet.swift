import Observation
import SwiftUI
import UIKit

/// The share dialog's state, read again from the Rust form after every change.
@MainActor
@Observable
private final class ShareFormModel {
    let form: ShareForm
    private(set) var state: ShareFormState

    init(form: ShareForm) {
        self.form = form
        state = form.state()
    }

    func apply(_ change: (ShareForm) throws -> Void) rethrows {
        try change(form)
        state = form.state()
    }
}

/// Shares files and folders with other users, or by a link anyone with it can open.
struct ShareSheet: View {
    let model: SessionModel
    let client: CoreClient
    let entries: [Entry]
    let owner: String
    @Environment(\.dismiss) private var dismiss
    @State private var share: ShareFormModel?
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Group {
                if let share {
                    ShareFormView(model: model, share: share, onClose: { dismiss() })
                } else if let error {
                    ContentUnavailableView("Can't Share", systemImage: "person.crop.circle.badge.xmark", description: Text(error))
                } else {
                    ProgressView()
                }
            }
            .toolbar {
                if share == nil {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { dismiss() }
                    }
                }
            }
        }
        .task {
            do {
                share = ShareFormModel(form: try await client.shareForm(entries: entries, owner: owner))
            } catch CoreError.Unauthorized {
                model.sessionEnded()
            } catch {
                self.error = fileMessage(for: error)
            }
        }
    }
}

private struct ShareFormView: View {
    let model: SessionModel
    let share: ShareFormModel
    let onClose: () -> Void
    @State private var query = ""
    @State private var password = ""
    @State private var date = Date().addingTimeInterval(7 * 24 * 3600)
    @State private var saving = false
    @State private var stopping = false
    @State private var askingStop = false
    @State private var copied = false
    @State private var error: String?

    private var state: ShareFormState { share.state }

    var body: some View {
        Form {
            Section {
                Picker("Share With", selection: Binding(
                    get: { state.audience },
                    set: { audience in share.apply { $0.setAudience(audience: audience) } }
                )) {
                    Text("People").tag(ShareAudience.people)
                    Text("Link").tag(ShareAudience.link)
                }
                .pickerStyle(.segmented)
                .listRowBackground(Color.clear)
                .listRowInsets(EdgeInsets())
            } footer: {
                Text(state.subtitle)
            }
            if let error {
                Text(error).foregroundStyle(.red)
            }
            switch state.audience {
            case .people: people
            case .link: link
            }
            expiry
            if state.hasShare {
                Section {
                    Button(stopping ? "Stopping…" : "Stop Sharing", role: .destructive) { askingStop = true }
                        .disabled(stopping)
                }
            }
        }
        .navigationTitle(state.title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel", action: onClose)
            }
            ToolbarItem(placement: .confirmationAction) {
                if saving {
                    ProgressView()
                } else {
                    Button("Save") { Task { await save() } }
                }
            }
        }
        .interactiveDismissDisabled(saving)
        .confirmationDialog(state.stopQuestion, isPresented: $askingStop, titleVisibility: .visible) {
            Button("Stop Sharing", role: .destructive) { Task { await stop() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(state.stopNote)
        }
    }

    // MARK: People

    @ViewBuilder
    private var people: some View {
        Section("People With Access") {
            Label(state.owner, systemImage: "person.crop.circle")
            ForEach(state.people, id: \.id) { person in
                Label(person.username, systemImage: "person.crop.circle")
                    .swipeActions {
                        Button("Remove", role: .destructive) { remove(person) }
                    }
                    .contextMenu {
                        Button("Remove", systemImage: "minus.circle", role: .destructive) { remove(person) }
                    }
            }
        }
        Section {
            TextField("Add people by name", text: $query)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            ForEach(share.form.candidates(query: query.trimmingCharacters(in: .whitespaces)), id: \.id) { person in
                Button {
                    share.apply { $0.addPerson(id: person.id) }
                    query = ""
                } label: {
                    Label(person.username, systemImage: "plus.circle")
                }
            }
        }
    }

    private func remove(_ person: SharePerson) {
        do {
            try share.apply { try $0.removePerson(id: person.id) }
        } catch {
            self.error = fileMessage(for: error)
        }
    }

    // MARK: Link

    @ViewBuilder
    private var link: some View {
        Section {
            if let url = state.url {
                Text(url)
                    .font(.callout.monospaced())
                    .textSelection(.enabled)
                    .lineLimit(2)
                Button(copied ? "Copied" : "Copy Link", systemImage: copied ? "checkmark" : "doc.on.doc") {
                    UIPasteboard.general.string = url
                    copied = true
                }
                if let shareURL = URL(string: url) {
                    ShareLink(item: shareURL) {
                        Label("Share Link…", systemImage: "square.and.arrow.up")
                    }
                }
            } else {
                Text("Save to make a link anyone who has it can open.")
                    .foregroundStyle(.secondary)
            }
        } header: {
            Text("Link")
        }
        Section {
            Toggle("Require Password", isOn: Binding(
                get: { state.requirePassword },
                set: { require in share.apply { $0.setRequirePassword(require: require) } }
            ))
            if state.requirePassword {
                if state.editingPassword {
                    SecureField("Password", text: $password)
                        .textContentType(.newPassword)
                        .onChange(of: password) {
                            share.apply { $0.setPassword(password: password) }
                        }
                } else if state.offersChangePassword {
                    Button("Change Password") {
                        password = ""
                        share.apply { $0.changePassword() }
                    }
                }
            }
        }
    }

    // MARK: Expiry

    private var expiry: some View {
        Section {
            Picker("Expires", selection: Binding(
                get: { state.expiry },
                set: { chip in
                    share.apply { $0.setExpiry(expiry: chip) }
                    if chip == .date {
                        share.apply { $0.pickDate(at: date) }
                    }
                }
            )) {
                Text("In a Day").tag(ShareExpiry.day)
                Text("In a Week").tag(ShareExpiry.week)
                Text("In a Month").tag(ShareExpiry.month)
                Text("Never").tag(ShareExpiry.never)
                Text("On a Date").tag(ShareExpiry.date)
            }
            if state.expiry == .date {
                DatePicker("Date", selection: $date, in: Date()..., displayedComponents: [.date, .hourAndMinute])
                    .onChange(of: date) {
                        share.apply { $0.pickDate(at: date) }
                    }
            }
        } footer: {
            if let at = state.expiresAt {
                Text("Stops working \(at.formatted(date: .abbreviated, time: .shortened)).")
            } else {
                Text("Works until you stop sharing.")
            }
        }
    }

    // MARK: Saving

    private func save() async {
        saving = true
        defer { saving = false }
        do {
            let saved = try await share.form.save()
            share.apply { _ in }
            error = nil
            if saved == .updated {
                onClose()
            }
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }

    private func stop() async {
        stopping = true
        defer { stopping = false }
        do {
            try await share.form.stop()
            onClose()
        } catch CoreError.Unauthorized {
            model.sessionEnded()
        } catch {
            self.error = fileMessage(for: error)
        }
    }
}
