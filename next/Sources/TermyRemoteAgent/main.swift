import Foundation
import TermyRemoteKit

/// `termy-remote`: signs this computer in to the Termy relay and serves its
/// terminal sessions to the account's phones, end-to-end encrypted.
let usage = """
usage: termy-remote <command>

  signup [email]        create an account and sign this computer in
  login [email]         sign this computer in
  logout                sign out and remove this computer from the account
  status                show the account, this computer's key and trusted phones
  devices               list the account's devices
  serve [--trust-new] [--shared]
                        serve Termy Next's sessions (--shared: termy mux's) to your phones
  untrust <device-id>   forget a phone; it is asked about again next time

Set TERMY_RELAY_URL to use another relay (default \(RelayAPI.defaultURL.absoluteString)).
"""

let store = AgentStore()

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("termy-remote: \(message)\n".utf8))
    exit(1)
}

func prompt(_ label: String) -> String {
    print(label, terminator: "")
    fflush(stdout)
    return readLine()?.trimmingCharacters(in: .whitespaces) ?? ""
}

func password(_ label: String) -> String {
    var buffer = [CChar](repeating: 0, count: 512)
    // Without a terminal (a script piping the password), read it from stdin.
    let flags = isatty(STDIN_FILENO) != 0 ? 0 : RPP_STDIN
    guard let line = readpassphrase(label, &buffer, buffer.count, flags) else { fail("couldn't read the password") }
    return String(cString: line)
}

var relayURL: URL {
    if let value = ProcessInfo.processInfo.environment["TERMY_RELAY_URL"], let url = URL(string: value) { return url }
    return store.credentials()?.relayURL ?? RelayAPI.defaultURL
}

func requireCredentials() -> AgentStore.Credentials {
    guard let credentials = store.credentials() else { fail("not signed in. Run `termy remote login`.") }
    return credentials
}

var hostName: String { Host.current().localizedName ?? ProcessInfo.processInfo.hostName }

@MainActor
func signIn(creating: Bool, email: String?) async {
    let email = email ?? prompt("Email: ")
    let secret = password("Password: ")
    if creating, password("Repeat password: ") != secret { fail("the passwords don't match") }
    do {
        let identity = try store.identity()
        let api = RelayAPI(baseURL: relayURL)
        let device = DeviceRegistration(name: hostName, kind: .host, publicKey: identity.publicKey)
        let response = creating
            ? try await api.signUp(email: email, password: secret, device: device)
            : try await api.logIn(email: email, password: secret, device: device)
        try store.save(.init(relayURL: relayURL, token: response.token, email: response.user.email, deviceID: response.device.id))
        print("Signed in as \(response.user.email) on \(relayURL.host ?? relayURL.absoluteString).")
        print("This computer's key: \(identity.fingerprint)")
        print("Run `termy remote serve` to open your sessions to the Termy app.")
    } catch {
        fail(error.localizedDescription)
    }
}

let arguments = Array(CommandLine.arguments.dropFirst())
switch arguments.first {
case "signup":
    await signIn(creating: true, email: arguments.dropFirst().first)
case "login":
    await signIn(creating: false, email: arguments.dropFirst().first)
case "logout":
    let credentials = requireCredentials()
    do {
        try await RelayAPI(baseURL: credentials.relayURL, token: credentials.token).logOut()
    } catch let error as RelayAPIError where error.status == 401 {
        // Already signed out on the relay.
    } catch {
        fail(error.localizedDescription)
    }
    store.clearCredentials()
    print("Signed out.")
case "status":
    let identity = (try? store.identity())?.fingerprint ?? "?"
    if let credentials = store.credentials() {
        print("Signed in as \(credentials.email) on \(credentials.relayURL.absoluteString)")
    } else {
        print("Not signed in.")
    }
    print("This computer's key: \(identity)")
    let trust = TrustStore(url: store.trustURL)
    print(trust.entries.isEmpty ? "No trusted phones yet." : "Trusted phones:")
    for (id, entry) in trust.entries.sorted(by: { $0.value.addedAt < $1.value.addedAt }) {
        print("  \(entry.name)  \(id)  \(Data(base64Encoded: entry.publicKey).map(Fingerprint.of) ?? "?")")
    }
case "devices":
    let credentials = requireCredentials()
    do {
        let devices = try await RelayAPI(baseURL: credentials.relayURL, token: credentials.token).devices()
        for device in devices {
            let me = device.id == credentials.deviceID ? " (this computer)" : ""
            print("\(device.online ? "●" : "○") \(device.name)\(me)  \(device.kind.rawValue)  \(device.id)")
            print("    key \(device.fingerprint)")
        }
    } catch {
        fail(error.localizedDescription)
    }
case "untrust":
    guard let raw = arguments.dropFirst().first, let id = UUID(uuidString: raw) else { fail("usage: termy-remote untrust <device-id>") }
    var trust = TrustStore(url: store.trustURL)
    do { try trust.remove(deviceID: id) } catch { fail(error.localizedDescription) }
    print("Forgot \(raw).")
case "serve", "up":
    let credentials = requireCredentials()
    var options = Agent.Options()
    options.trustNew = arguments.contains("--trust-new")
    if !arguments.contains("--shared") {
        // Termy Next keeps its own host; see next/Sources/TermyNext/SessionHost.swift.
        if let home = ProcessInfo.processInfo.environment["TERMY_NEXT_HOME"] {
            options.muxRoot = URL(fileURLWithPath: home).appendingPathComponent("support/multiplexer", isDirectory: true)
        } else {
            let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            options.muxRoot = support.appendingPathComponent("TermyNext/multiplexer", isDirectory: true)
        }
    }
    do {
        let agent = Agent(store: store, credentials: credentials, identity: try store.identity(), options: options)
        try await agent.run()
    } catch {
        fail("\(error)")
    }
case "-h", "--help", "help", nil:
    print(usage)
default:
    fail("unknown command \(arguments[0]).\n\n\(usage)")
}
