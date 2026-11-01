// swift-tools-version: 6.0
import PackageDescription

// Termy Next: a session-first macOS terminal built on libtermy through the
// Swift SDK in ../sdk/swift. Stage libtermy first with
// `../sdk/swift/scripts/build-libtermy.sh`, then `swift run TermyNext` or
// `scripts/bundle.sh` for a .app.
let package = Package(
    name: "TermyNext",
    platforms: [.macOS(.v14)],
    dependencies: [
        .package(path: "../sdk/swift"),
    ],
    targets: [
        // Layout tree, fuzzy matching and naming: no AppKit, no libtermy.
        .target(name: "TermyNextCore"),
        .executableTarget(
            name: "TermyNext",
            dependencies: [
                "TermyNextCore",
                .product(name: "Termy", package: "swift"),
                .product(name: "TermyAppKit", package: "swift"),
            ],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "TermyNextCoreTests",
            dependencies: ["TermyNextCore"]
        ),
    ]
)
