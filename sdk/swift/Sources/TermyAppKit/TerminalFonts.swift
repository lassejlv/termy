#if os(macOS)
import AppKit
import CoreText
import Termy

/// The configured font in four styles, plus glyph lookup with system fallback.
///
/// Both renderers resolve glyphs here, so CoreGraphics and Metal agree on which
/// font draws each cell.
@MainActor
final class TerminalFonts {
    struct Style: Hashable {
        var bold: Bool
        var italic: Bool

        static let regular = Style(bold: false, italic: false)
    }

    /// A glyph in a specific font. `fontIndex` identifies the font for caching.
    struct Glyph: Hashable {
        var fontIndex: Int
        var glyph: CGGlyph
        /// Drawn from a color font such as Apple Color Emoji.
        var isColor: Bool
    }

    private struct LookupKey: Hashable {
        var codepoint: UInt32
        var style: Style
    }

    let fontFamily: String
    let fontSize: CGFloat
    private(set) var fonts: [CTFont] = []
    private var styleIndices: [Style: Int] = [:]
    private var fallbackIndices: [String: Int] = [:]
    private var lookups: [LookupKey: Glyph?] = [:]

    init(fontFamily: String, fontSize: Float) {
        self.fontFamily = fontFamily
        self.fontSize = CGFloat(fontSize)
    }

    var ascent: CGFloat { CTFontGetAscent(font(.regular)) }
    var descent: CGFloat { CTFontGetDescent(font(.regular)) }
    var underlinePosition: CGFloat { -CTFontGetUnderlinePosition(font(.regular)) }

    /// Distance from a cell's top edge to the text baseline, in points.
    func baselineOffset(cellHeight: CGFloat) -> CGFloat {
        ((cellHeight - (ascent + descent)) / 2 + ascent).rounded()
    }

    func font(_ style: Style) -> CTFont {
        fonts[fontIndex(style)]
    }

    func font(at index: Int) -> CTFont {
        fonts[index]
    }

    /// The glyph that draws `codepoint`, falling back to a system font when the
    /// configured one lacks it. Nil for blanks and unrenderable scalars.
    func glyph(for codepoint: UInt32, style: Style) -> Glyph? {
        let key = LookupKey(codepoint: codepoint, style: style)
        if let cached = lookups[key] { return cached }
        let resolved = resolve(codepoint, style: style)
        lookups[key] = resolved
        return resolved
    }

    private func resolve(_ codepoint: UInt32, style: Style) -> Glyph? {
        guard let scalar = Unicode.Scalar(codepoint) else { return nil }
        let string = String(Character(scalar))
        let utf16 = Array(string.utf16)
        let primaryIndex = fontIndex(style)
        if let glyph = lookUp(utf16, in: fonts[primaryIndex]) {
            return Glyph(fontIndex: primaryIndex, glyph: glyph, isColor: false)
        }
        // Icons in the private use areas (Nerd Font, Powerline) come from an
        // installed Nerd Font first: the system's fallback doesn't look there
        // and ends at LastResort, which draws a boxed question mark.
        let privateUse = (0xE000...0xF8FF).contains(codepoint) || codepoint >= 0xF0000
        var fallback = CTFontCreateForString(fonts[primaryIndex], string as CFString, CFRange(location: 0, length: utf16.count))
        var glyph = lookUp(utf16, in: fallback)
        if privateUse || glyph == nil || Self.isLastResort(fallback),
           let symbol = Self.symbolFonts(size: fontSize).lazy.compactMap({ font in self.lookUp(utf16, in: font).map { (font, $0) } }).first {
            fallback = symbol.0
            glyph = symbol.1
        }
        guard let glyph else { return nil }
        let name = "\(CTFontCopyPostScriptName(fallback))|\(style.bold)|\(style.italic)"
        let index: Int
        if let existing = fallbackIndices[name] {
            index = existing
        } else {
            index = fonts.count
            fonts.append(fallback)
            fallbackIndices[name] = index
        }
        let isColor = CTFontGetSymbolicTraits(fallback).contains(.traitColorGlyphs)
        return Glyph(fontIndex: index, glyph: glyph, isColor: isColor)
    }

    private static func isLastResort(_ font: CTFont) -> Bool {
        (CTFontCopyPostScriptName(font) as String).hasPrefix("LastResort")
    }

    /// Installed fonts that carry icon glyphs, best first: the Nerd Font
    /// symbols font, other Nerd Fonts (monospaced ones first), then Apple
    /// Symbols. Looked up once per size.
    private static var symbolFontCache: [CGFloat: [CTFont]] = [:]
    private static let symbolFamilies: [String] = {
        let families = NSFontManager.shared.availableFontFamilies
        func rank(_ family: String) -> Int? {
            let name = family.lowercased()
            if name.hasPrefix("symbols nerd font") { return name.contains("mono") ? 0 : 1 }
            if name.contains("nerd font") { return name.contains("propo") ? 4 : name.contains("mono") ? 2 : 3 }
            if name == "apple symbols" { return 5 }
            return nil
        }
        return families.compactMap { family in rank(family).map { ($0, family) } }
            .sorted { $0.0 != $1.0 ? $0.0 < $1.0 : $0.1 < $1.1 }
            .map(\.1)
    }()

    static func symbolFonts(size: CGFloat) -> [CTFont] {
        if let cached = symbolFontCache[size] { return cached }
        let fonts = symbolFamilies.compactMap { family -> CTFont? in
            NSFontManager.shared.font(withFamily: family, traits: [], weight: 5, size: size)
        }
        symbolFontCache[size] = fonts
        return fonts
    }

    private func lookUp(_ utf16: [UniChar], in font: CTFont) -> CGGlyph? {
        var glyphs = [CGGlyph](repeating: 0, count: utf16.count)
        guard CTFontGetGlyphsForCharacters(font, utf16, &glyphs, utf16.count), glyphs[0] != 0 else { return nil }
        return glyphs[0]
    }

    private func fontIndex(_ style: Style) -> Int {
        if let index = styleIndices[style] { return index }
        let base = NSFont(name: fontFamily, size: fontSize)
            ?? NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        var traits: NSFontTraitMask = []
        if style.bold { traits.insert(.boldFontMask) }
        if style.italic { traits.insert(.italicFontMask) }
        let styled = traits.isEmpty ? base : NSFontManager.shared.convert(base, toHaveTrait: traits)
        let index = fonts.count
        fonts.append(styled)
        styleIndices[style] = index
        return index
    }
}
#endif
