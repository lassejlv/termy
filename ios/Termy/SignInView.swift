import SwiftUI
import TermyRemoteKit

struct SignInView: View {
    @Environment(AppModel.self) private var model
    @State private var email = ""
    @State private var password = ""
    @State private var creating = false
    @State private var busy = false
    @State private var error: String?
    @State private var relay = ""
    @FocusState private var focus: Field?

    enum Field { case email, password }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    VStack(spacing: 10) {
                        Image("Logo")
                            .resizable()
                            .frame(width: 84, height: 84)
                            .clipShape(RoundedRectangle(cornerRadius: 19, style: .continuous))
                        Text("Termy")
                            .font(.largeTitle.bold())
                        Text("Your Mac's terminal sessions, end-to-end encrypted.")
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                    }
                    .frame(maxWidth: .infinity)
                    .listRowBackground(Color.clear)
                }

                Section {
                    TextField("Email", text: $email)
                        .textContentType(.username)
                        .keyboardType(.emailAddress)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .focused($focus, equals: .email)
                        .submitLabel(.next)
                        .onSubmit { focus = .password }
                    SecureField("Password", text: $password)
                        .textContentType(creating ? .newPassword : .password)
                        .focused($focus, equals: .password)
                        .submitLabel(.go)
                        .onSubmit(submit)
                } footer: {
                    if creating { Text("At least 8 characters.") }
                }

                if let error {
                    Section { Text(error).foregroundStyle(.red) }
                }

                Section {
                    Button(action: submit) {
                        HStack {
                            Spacer()
                            if busy { ProgressView() } else { Text(creating ? "Create Account" : "Sign In").bold() }
                            Spacer()
                        }
                    }
                    .disabled(busy || email.isEmpty || password.isEmpty)
                    Button(creating ? "I have an account" : "Create an account") {
                        creating.toggle()
                        error = nil
                    }
                    .frame(maxWidth: .infinity)
                }

                Section {
                    DisclosureGroup("Relay server") {
                        TextField(RelayAPI.defaultURL.absoluteString, text: $relay)
                            .keyboardType(.URL)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .onSubmit(applyRelay)
                    }
                } footer: {
                    Text("Sign in with the same account on your Mac with `termy remote login`.")
                }
            }
            .onAppear { relay = model.relayURL.absoluteString }
        }
    }

    private func applyRelay() {
        if let url = URL(string: relay.trimmingCharacters(in: .whitespaces)), url.scheme?.hasPrefix("http") == true {
            model.relayURL = url
        } else {
            relay = model.relayURL.absoluteString
        }
    }

    private func submit() {
        guard !busy, !email.isEmpty, !password.isEmpty else { return }
        applyRelay()
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await model.signIn(email: email.trimmingCharacters(in: .whitespaces), password: password, creating: creating)
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}
