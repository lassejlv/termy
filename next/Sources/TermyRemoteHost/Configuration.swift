import Foundation
import Termy
import TermyRemoteKit

extension RemoteConfiguration {
    public init(render: RenderConfig, contents: String, reduceMotion: Bool = false, appearance: Appearance = .dark) {
        self.init()
        self.contents = contents
        self.appearance = appearance == .light ? "light" : "dark"
        fontFamily = render.fontFamily
        fontSize = Double(render.fontSize)
        cellWidth = Double(render.cellWidth)
        cellHeight = Double(render.cellHeight)
        paddingX = Double(render.paddingX)
        paddingY = Double(render.paddingY)
        foreground = render.foreground.rgba
        background = render.background.rgba
        cursorColor = render.cursorColor.rgba
        cursorStyle = render.cursorStyle == .bar ? "bar" : "block"
        cursorBlink = render.cursorBlink
        theme = render.activeTheme
        scrollMultiplier = Double(render.mouseScrollMultiplier)
        copyOnSelect = render.copyOnSelect
        self.reduceMotion = reduceMotion
    }
}
