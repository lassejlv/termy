// swift-tools-version: 6.0
import PackageDescription

// TermyRemoteKit: what the Termy iPhone app and the Termy remote host
// share to reach terminal sessions. The phone connects to the Mac directly;
// everything a session shows or receives is end-to-end encrypted between the
// two devices (see README.md).
let package = Package(
    name: "TermyRemote",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(name: "TermyRemoteKit", targets: ["TermyRemoteKit"]),
    ],
    targets: [
        .target(name: "TermyRemoteKit"),
        .testTarget(name: "TermyRemoteKitTests", dependencies: ["TermyRemoteKit"]),
    ]
)
