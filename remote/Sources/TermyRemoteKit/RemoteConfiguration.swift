import Foundation

/// Resolved Mac settings plus the original config, transferred only inside SecureChannel.
/// The phone applies terminal settings; Mac-only settings stay with the saved copy.
public struct RemoteConfiguration: Codable, Hashable, Sendable {
    public var contents = ""
    public var fontFamily = "monospace"
    public var fontSize: Double = 13
    public var cellWidth: Double = 8
    public var cellHeight: Double = 17
    public var paddingX: Double = 8
    public var paddingY: Double = 8
    public var foreground: UInt32 = 0xffff_ffff
    public var background: UInt32 = 0x1014_21ff
    public var cursorColor: UInt32 = 0xffff_ffff
    public var cursorStyle = "block"
    public var cursorBlink = false
    public var theme = ""
    public var appearance = "dark"
    public var scrollMultiplier: Double = 1
    public var copyOnSelect = false
    public var reduceMotion = false
    public init() {}
}
