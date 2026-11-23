internal import CTermy

/// The libtermy C ABI check. The SDK compiles against termy.h, but the
/// library is loaded at run time, and a libtermy built from another header
/// can lay out its structs differently.
public enum TermyABI {
    /// Stops the process with a clear message unless the loaded libtermy has
    /// the ABI version of the termy.h this SDK was compiled against. Creating
    /// a terminal or connecting a multiplexer checks it; apps call it at
    /// launch to fail before anything else touches the library.
    public static func requireCompatibleLibrary() {
        _ = checked
    }

    private static let checked: Void = {
        let library = termy_abi_version()
        let header = UInt32(TERMY_ABI_VERSION)
        guard library == header else {
            fatalError(
                "libtermy has ABI version \(library), but the Termy SDK was compiled against termy.h version \(header). "
                    + "Rebuild libtermy (sdk/swift/scripts/build-libtermy.sh) so it matches the header."
            )
        }
    }()
}
