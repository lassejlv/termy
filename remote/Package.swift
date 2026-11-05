// swift-tools-version: 6.0
import PackageDescription

// TermyRemoteKit: what the Termy iPhone app and the Termy remote host
// share to reach terminal sessions through the relay. The relay only routes
// sealed bytes; everything a session shows or receives is end-to-end
// encrypted between the two devices (see Sources/TermyRemoteKit/README.md).
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
