#if os(macOS)
import AppKit
import CoreText
import Metal
import QuartzCore
import Termy

/// Draws a retained terminal frame with Metal.
///
/// Every cell becomes instanced quads: solid backgrounds and decorations, and
/// glyphs sampled from a `GlyphAtlas`. Quads are cached per row and rebuilt
/// only for rows the view invalidates; the cursor, selection, images and IME
/// text are rebuilt every frame. A frame is a handful of draw calls.
@MainActor
final class MetalTerminalRenderer {
    private struct RowQuads {
        /// Cell backgrounds, drawn below selection and images.
        var backgrounds: [MetalQuad] = []
        /// Glyphs, decorations and special-glyph geometry.
        var foreground: [MetalQuad] = []
    }

    private struct Batch {
        var texture: MTLTexture
        var range: Range<Int>
        var scissor: MTLScissorRect?
    }

    private struct Frame {
        var quads: [MetalQuad] = []
        var batches: [Batch] = []

        mutating func append(_ newQuads: [MetalQuad], texture: MTLTexture, scissor: MTLScissorRect? = nil) {
            guard !newQuads.isEmpty else { return }
            let start = quads.count
            quads.append(contentsOf: newQuads)
            if scissor == nil, let last = batches.last, last.scissor == nil, last.texture === texture,
               last.range.upperBound == start {
                batches[batches.count - 1].range = last.range.lowerBound..<quads.count
            } else {
                batches.append(Batch(texture: texture, range: start..<quads.count, scissor: scissor))
            }
        }
    }

    static let maxFramesInFlight = 3

    let device: MTLDevice
    private let queue: MTLCommandQueue
    private let pipeline: MTLRenderPipelineState
    private let atlas: GlyphAtlas
    private let inFlight = DispatchSemaphore(value: maxFramesInFlight)
    private var buffers: [MTLBuffer?] = Array(repeating: nil, count: maxFramesInFlight)
    private var bufferIndex = 0

    private var config: RenderConfig?
    private var scale: CGFloat = 0
    private var fonts = TerminalFonts(fontFamily: "", fontSize: 13)
    private var baselineOffset: CGFloat = 0
    private var rows: [RowQuads?] = []
    private var rowCols = 0
    private var atlasGeneration = 0
    private var imageTextures: [String: MTLTexture] = [:]
    /// Images whose texture couldn't be created, so they aren't retried every frame.
    private var failedImages: Set<String> = []
    /// The largest texture side this GPU supports. Larger images are downscaled.
    let maxTextureDimension: Int
    private var markedText: (key: String, texture: MTLTexture, width: Int)?

    /// Nil when the system has no Metal device or the shaders fail to compile.
    init?(device: MTLDevice? = MTLCreateSystemDefaultDevice()) {
        guard let device, let queue = device.makeCommandQueue(), let atlas = GlyphAtlas(device: device),
              let pipeline = try? Self.makePipeline(device: device) else { return nil }
        self.device = device
        maxTextureDimension = device.supportsFamily(.apple3) || device.supportsFamily(.mac2) ? 16_384 : 8_192
        self.queue = queue
        self.pipeline = pipeline
        self.atlas = atlas
    }

    private static func makePipeline(device: MTLDevice) throws -> MTLRenderPipelineState {
        let library = try device.makeLibrary(source: metalShaderSource, options: nil)
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.label = "Termy quads"
        descriptor.vertexFunction = library.makeFunction(name: "quad_vertex")
        descriptor.fragmentFunction = library.makeFunction(name: "quad_fragment")
        let attachment = descriptor.colorAttachments[0]!
        attachment.pixelFormat = .bgra8Unorm
        // Premultiplied source-over.
        attachment.isBlendingEnabled = true
        attachment.sourceRGBBlendFactor = .one
        attachment.sourceAlphaBlendFactor = .one
        attachment.destinationRGBBlendFactor = .oneMinusSourceAlpha
        attachment.destinationAlphaBlendFactor = .oneMinusSourceAlpha
        return try device.makeRenderPipelineState(descriptor: descriptor)
    }

    /// Prepares a layer to receive frames.
    func configure(_ layer: CAMetalLayer) {
        layer.device = device
        layer.pixelFormat = .bgra8Unorm
        layer.colorspace = CGColorSpace(name: CGColorSpace.sRGB)
        layer.framebufferOnly = true
        // Present in step with Core Animation so live resizes don't stretch.
        layer.presentsWithTransaction = true
    }

    // MARK: - Invalidation

    /// Marks rows whose cells or special-glyph geometry changed.
    func invalidate(rows changed: some Sequence<Int>) {
        for row in changed where rows.indices.contains(row) { rows[row] = nil }
    }

    func invalidateAll() {
        rows = Array(repeating: nil, count: rows.count)
    }

    private func prepare(_ config: RenderConfig, scale: CGFloat, frame: FrameStore) {
        if config != self.config || scale != self.scale {
            self.config = config
            self.scale = scale
            fonts = TerminalFonts(fontFamily: config.fontFamily, fontSize: config.fontSize)
            baselineOffset = fonts.baselineOffset(cellHeight: CGFloat(config.cellHeight))
            atlas.reset()
            markedText = nil
            rows = []
        }
        if rows.count != frame.rows || rowCols != frame.cols {
            rows = Array(repeating: nil, count: frame.rows)
            rowCols = frame.cols
        }
        if atlasGeneration != atlas.generation {
            atlasGeneration = atlas.generation
            invalidateAll()
        }
    }

    // MARK: - Drawing

    /// Renders into the layer's next drawable and presents it.
    func draw(_ state: RenderState, in layer: CAMetalLayer) {
        let scale = layer.contentsScale
        guard layer.drawableSize.width >= 1, layer.drawableSize.height >= 1,
              let commandBuffer = queue.makeCommandBuffer() else { return }
        inFlight.wait()
        let semaphore = inFlight
        commandBuffer.addCompletedHandler { _ in semaphore.signal() }
        guard let drawable = layer.nextDrawable() else {
            commandBuffer.commit()
            return
        }
        encode(state, scale: scale, target: drawable.texture, commandBuffer: commandBuffer)
        commandBuffer.commit()
        commandBuffer.waitUntilScheduled()
        drawable.present()
    }

    /// Renders into an offscreen texture and returns its BGRA bytes, top row
    /// first. For tests and snapshots.
    func renderPixels(_ state: RenderState, width: Int, height: Int, scale: CGFloat) -> [UInt8]? {
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: width, height: height, mipmapped: false)
        descriptor.usage = [.renderTarget, .shaderRead]
        descriptor.storageMode = device.hasUnifiedMemory ? .shared : .managed
        guard let target = device.makeTexture(descriptor: descriptor),
              let commandBuffer = queue.makeCommandBuffer() else { return nil }
        inFlight.wait()
        let semaphore = inFlight
        commandBuffer.addCompletedHandler { _ in semaphore.signal() }
        encode(state, scale: scale, target: target, commandBuffer: commandBuffer)
        if target.storageMode == .managed, let blit = commandBuffer.makeBlitCommandEncoder() {
            blit.synchronize(resource: target)
            blit.endEncoding()
        }
        commandBuffer.commit()
        commandBuffer.waitUntilCompleted()
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        target.getBytes(&pixels, bytesPerRow: width * 4, from: MTLRegionMake2D(0, 0, width, height), mipmapLevel: 0)
        return pixels
    }

    private func encode(_ state: RenderState, scale: CGFloat, target: MTLTexture, commandBuffer: MTLCommandBuffer) {
        let config = state.config
        prepare(config, scale: scale, frame: state.frame)
        var frame = buildFrame(state)
        if atlasGeneration != atlas.generation {
            // The atlas filled up at its largest size mid-frame; rebuild
            // against the fresh one. Glyphs that still don't fit are skipped
            // rather than starting over again, which would leave rows built
            // earlier in this pass pointing into the replaced texture.
            prepare(config, scale: scale, frame: state.frame)
            atlas.allowsReset = false
            frame = buildFrame(state)
            atlas.allowsReset = true
        }

        let pass = MTLRenderPassDescriptor()
        let opacity = Double(config.backgroundOpacity)
        pass.colorAttachments[0].texture = target
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColor(
            red: Double(config.background.r) / 255 * opacity,
            green: Double(config.background.g) / 255 * opacity,
            blue: Double(config.background.b) / 255 * opacity,
            alpha: opacity)
        guard let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: pass) else { return }
        defer { encoder.endEncoding() }
        guard !frame.quads.isEmpty, let buffer = buffer(for: frame.quads) else { return }

        encoder.setRenderPipelineState(pipeline)
        encoder.setVertexBuffer(buffer, offset: 0, index: 0)
        var viewport = SIMD2<Float>(Float(target.width), Float(target.height))
        encoder.setVertexBytes(&viewport, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
        let fullScissor = MTLScissorRect(x: 0, y: 0, width: target.width, height: target.height)
        for batch in frame.batches {
            encoder.setFragmentTexture(batch.texture, index: 0)
            encoder.setScissorRect(batch.scissor.map { clamp($0, to: fullScissor) } ?? fullScissor)
            encoder.drawPrimitives(
                type: .triangleStrip, vertexStart: 0, vertexCount: 4,
                instanceCount: batch.range.count, baseInstance: batch.range.lowerBound)
        }
    }

    private func buffer(for quads: [MetalQuad]) -> MTLBuffer? {
        let length = quads.count * MemoryLayout<MetalQuad>.stride
        bufferIndex = (bufferIndex + 1) % Self.maxFramesInFlight
        if let existing = buffers[bufferIndex], existing.length >= length {
            quads.withUnsafeBytes { existing.contents().copyMemory(from: $0.baseAddress!, byteCount: length) }
            return existing
        }
        // Grow with headroom so steady output doesn't reallocate.
        guard let buffer = device.makeBuffer(length: max(length * 3 / 2, 64 * 1024), options: .storageModeShared)
        else { return nil }
        quads.withUnsafeBytes { buffer.contents().copyMemory(from: $0.baseAddress!, byteCount: length) }
        buffers[bufferIndex] = buffer
        return buffer
    }

    private func clamp(_ scissor: MTLScissorRect, to bounds: MTLScissorRect) -> MTLScissorRect {
        let x = min(max(scissor.x, 0), bounds.width)
        let y = min(max(scissor.y, 0), bounds.height)
        return MTLScissorRect(
            x: x, y: y,
            width: max(0, min(scissor.x + scissor.width, bounds.width) - x),
            height: max(0, min(scissor.y + scissor.height, bounds.height) - y))
    }

    // MARK: - Frame assembly

    private func buildFrame(_ state: RenderState) -> Frame {
        var frame = Frame()
        let store = state.frame
        guard !store.isEmpty else { return frame }
        for row in 0..<store.rows where rows[row] == nil {
            rows[row] = buildRow(row, state: state)
        }
        let texture = atlas.texture
        frame.append(rows.flatMap { $0?.backgrounds ?? [] }, texture: texture)
        frame.append(selectionQuads(state), texture: texture)
        var liveImages: [String: MTLTexture] = [:]
        var failed: Set<String> = []
        appendImages(state.images.filter { $0.placement.zIndex < 0 }, to: &frame, live: &liveImages, failed: &failed)
        frame.append(rows.flatMap { $0?.foreground ?? [] }, texture: texture)
        appendImages(state.images.filter { $0.placement.zIndex >= 0 }, to: &frame, live: &liveImages, failed: &failed)
        imageTextures = liveImages
        failedImages = failed
        frame.append(cursorQuads(state), texture: atlas.texture)
        if let marked = state.markedText, !marked.isEmpty {
            appendMarkedText(marked, state: state, to: &frame)
        }
        return frame
    }

    private func buildRow(_ row: Int, state: RenderState) -> RowQuads {
        let store = state.frame
        let config = state.config
        var quads = RowQuads()
        let cellAlpha = config.backgroundOpacityAffectsCells ? config.backgroundOpacity : 1

        var col = 0
        while col < store.cols {
            let cell = store[row, col]
            guard !cell.usesDefaultBackground else { col += 1; continue }
            var end = col + 1
            while end < store.cols {
                let next = store[row, end]
                guard !next.usesDefaultBackground, next.background == cell.background else { break }
                end += 1
            }
            quads.backgrounds.append(solid(cellPixels(row: row, col: col, width: end - col), cell.background, alpha: cellAlpha))
            col = end
        }

        let baseline = pixel(CGFloat(config.paddingY) + CGFloat(row) * cellHeight + baselineOffset)
        for col in 0..<store.cols {
            let cell = store[row, col]
            if let entry = state.glyphPlan[row * store.cols + col] {
                appendGeometry(entry, cell: cell, row: row, col: col, to: &quads.foreground)
            } else if cell.text != nil, let quad = glyphQuad(cell, color: cell.foreground, row: row, col: col, baseline: baseline) {
                quads.foreground.append(quad)
            }
            guard cell.isUnderlined || cell.isStrikethrough, !cell.isWideCharacterSpacer else { continue }
            let rect = cellPixels(row: row, col: col)
            let thickness = max(1, scale.rounded())
            if cell.isUnderlined {
                let y = (baseline + max(fonts.underlinePosition * scale, thickness)).rounded()
                quads.foreground.append(solid(SIMD4(rect.x, Float(y), rect.z, Float(thickness)), cell.foreground))
            }
            if cell.isStrikethrough {
                let y = (baseline - fonts.ascent * 0.35 * scale).rounded()
                quads.foreground.append(solid(SIMD4(rect.x, Float(y), rect.z, Float(thickness)), cell.foreground))
            }
        }
        return quads
    }

    private func glyphQuad(_ cell: Cell, color: TermyColor, row: Int, col: Int, baseline: CGFloat) -> MetalQuad? {
        guard let glyph = fonts.glyph(for: cell.codepoint, style: .init(bold: cell.isBold, italic: cell.isItalic))
        else { return nil }
        let font = fonts.font(at: glyph.fontIndex)
        let scale = self.scale
        guard let entry = atlas.entry(for: .glyph(glyph), rasterize: {
            GlyphRasterizer.glyph(glyph.glyph, font: font, scale: scale)
        }) else { return nil }
        let penX = pixel(CGFloat(config?.paddingX ?? 0) + CGFloat(col) * cellWidth)
        let origin = SIMD2(Float(penX), Float(baseline)) + entry.offset
        return MetalQuad(
            rect: SIMD4(origin.x, origin.y, entry.rect.z, entry.rect.w),
            uv: SIMD4(entry.rect.x, entry.rect.y, entry.rect.x + entry.rect.z, entry.rect.y + entry.rect.w),
            color: rgba(color), mode: glyph.isColor ? .colorGlyph : .mask)
    }

    private func appendGeometry(_ entry: GlyphPlanEntry, cell: Cell, row: Int, col: Int, to quads: inout [MetalQuad]) {
        let left = CGFloat(config?.paddingX ?? 0) * scale + CGFloat(col) * cellWidth * scale
        let top = CGFloat(config?.paddingY ?? 0) * scale + CGFloat(row) * cellHeight * scale
        let width = cellWidth * scale
        let height = cellHeight * scale
        for rect in entry.rects {
            func snap(_ value: CGFloat, up: Bool) -> CGFloat {
                rect.snap == .outward ? value.rounded(up ? .up : .down) : value.rounded()
            }
            let x0 = snap(left + CGFloat(rect.left) * width, up: false)
            let y0 = snap(top + CGFloat(rect.top) * height, up: false)
            let x1 = snap(left + CGFloat(rect.right) * width, up: true)
            let y1 = snap(top + CGFloat(rect.bottom) * height, up: true)
            quads.append(solid(
                SIMD4(Float(x0), Float(y0), Float(max(x1 - x0, 1)), Float(max(y1 - y0, 1))),
                cell.foreground, alpha: rect.alpha))
        }
        guard !entry.strokes.isEmpty else { return }
        let size = CGSize(width: width, height: height)
        guard let stroke = atlas.entry(for: .strokes(entry.strokes), rasterize: {
            GlyphRasterizer.strokes(entry.strokes, cellSize: size)
        }) else { return }
        let origin = SIMD2(Float(left.rounded()), Float(top.rounded())) + stroke.offset
        quads.append(MetalQuad(
            rect: SIMD4(origin.x, origin.y, stroke.rect.z, stroke.rect.w),
            uv: SIMD4(stroke.rect.x, stroke.rect.y, stroke.rect.x + stroke.rect.z, stroke.rect.y + stroke.rect.w),
            color: rgba(cell.foreground), mode: .mask))
    }

    private func selectionQuads(_ state: RenderState) -> [MetalQuad] {
        guard let selection = state.selection?.expanded(in: state.frame) else { return [] }
        let color = NSColor.selectedTextBackgroundColor.usingColorSpace(.sRGB) ?? .selectedTextBackgroundColor
        let rgba = SIMD4(Float(color.redComponent), Float(color.greenComponent), Float(color.blueComponent), 0.55)
        return selection.rows.compactMap { row in
            guard row >= 0, row < state.frame.rows else { return nil }
            let from = row == selection.start.row ? selection.start.col : 0
            let to = row == selection.end.row ? selection.end.col : state.frame.cols - 1
            guard from <= to else { return nil }
            return MetalQuad(rect: cellPixels(row: row, col: from, width: to - from + 1), color: rgba, mode: .solid)
        }
    }

    private func cursorQuads(_ state: RenderState) -> [MetalQuad] {
        let store = state.frame
        let cursor = store.cursor
        guard cursor.isVisible, state.cursorBlinkVisible || !state.isFocused,
              cursor.row < store.rows, cursor.col < store.cols else { return [] }
        let config = state.config
        let wide = cursor.col + 1 < store.cols && store[cursor.row, cursor.col + 1].isWideCharacterSpacer
        let rect = cellPixels(row: cursor.row, col: cursor.col, width: wide ? 2 : 1)
        let line = Float(max(1, scale.rounded()))

        if cursor.style == .bar {
            return [solid(SIMD4(rect.x, rect.y, Float((2 * scale).rounded()), rect.w), config.cursorColor)]
        }
        guard state.isFocused else {
            // Hollow block for an unfocused view.
            return [
                solid(SIMD4(rect.x, rect.y, rect.z, line), config.cursorColor),
                solid(SIMD4(rect.x, rect.y + rect.w - line, rect.z, line), config.cursorColor),
                solid(SIMD4(rect.x, rect.y, line, rect.w), config.cursorColor),
                solid(SIMD4(rect.x + rect.z - line, rect.y, line, rect.w), config.cursorColor),
            ]
        }
        // Filled block, then the cell's glyph again in the background color.
        var quads = [solid(rect, config.cursorColor)]
        let cell = store[cursor.row, cursor.col]
        if cell.text != nil, state.glyphPlan[cursor.row * store.cols + cursor.col] == nil {
            let baseline = pixel(CGFloat(config.paddingY) + CGFloat(cursor.row) * cellHeight + baselineOffset)
            let color = cell.usesDefaultBackground ? config.background : cell.background
            if let glyph = glyphQuad(cell, color: color, row: cursor.row, col: cursor.col, baseline: baseline) {
                quads.append(glyph)
            }
        }
        return quads
    }

    private func appendImages(
        _ images: [KittyImage], to frame: inout Frame, live: inout [String: MTLTexture], failed: inout Set<String>
    ) {
        for image in images.sorted(by: { $0.placement.zIndex < $1.placement.zIndex }) {
            let placement = image.placement
            let key = "\(placement.imageID):\(placement.imageGeneration)"
            if failed.contains(key) || failedImages.contains(key) {
                failed.insert(key)
                continue
            }
            guard let texture = live[key] ?? imageTextures[key] ?? makeTexture(image.image) else {
                failed.insert(key)
                continue
            }
            live[key] = texture
            // One quad covering only the visible part, sampling the matching
            // part of the source, so placeholder cells and partly clipped
            // images need no scissor and batch with each other.
            let geometry = KittyPlacementGeometry(placement, cellWidth: cellWidth, cellHeight: cellHeight)
            let visible = geometry.visible
            guard !visible.isNull, visible.width > 0, visible.height > 0 else { continue }
            let padding = CGPoint(x: CGFloat(config?.paddingX ?? 0), y: CGFloat(config?.paddingY ?? 0))
            let target = pixelRect(visible.offsetBy(dx: padding.x, dy: padding.y))
            guard target.z > 0, target.w > 0 else { continue }
            // Source coordinates are in image pixels; the texture may be downscaled.
            let source = geometry.source(for: visible, crop: placement.sourceCrop)
            let texelX = Float(texture.width) / Float(max(image.image.width, 1))
            let texelY = Float(texture.height) / Float(max(image.image.height, 1))
            let uv = SIMD4(
                Float(source.minX) * texelX, Float(source.minY) * texelY,
                Float(source.maxX) * texelX, Float(source.maxY) * texelY)
            frame.append([MetalQuad(rect: target, uv: uv, color: SIMD4(1, 1, 1, 1), mode: .image)], texture: texture)
        }
    }

    private func appendMarkedText(_ text: String, state: RenderState, to frame: inout Frame) {
        let config = state.config
        let cursor = state.frame.cursor
        let key = "\(text)|\(config.foreground.rgba)"
        if markedText?.key != key {
            let attributes: [NSAttributedString.Key: Any] = [
                .init(kCTFontAttributeName as String): fonts.font(.regular),
                .init(kCTForegroundColorAttributeName as String): CGColor(gray: 1, alpha: 1),
            ]
            let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
            let width = min(
                Int((CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil)) * scale).rounded(.up)), maxTextureDimension)
            let height = Int((cellHeight * scale).rounded(.up))
            guard let raster = GlyphRasterizer.line(line, width: width, height: height, baseline: baselineOffset, scale: scale),
                  let texture = makeTexture(raster) else { return }
            markedText = (key, texture, width)
        }
        guard let marked = markedText else { return }
        let cell = cellPixels(row: cursor.row, col: cursor.col)
        let width = Float(marked.width)
        frame.append([solid(SIMD4(cell.x, cell.y, width, cell.w), config.background)], texture: atlas.texture)
        frame.append(
            [MetalQuad(
                rect: SIMD4(cell.x, cell.y, width, Float(marked.texture.height)),
                uv: SIMD4(0, 0, width, Float(marked.texture.height)),
                color: rgba(config.foreground), mode: .mask)],
            texture: marked.texture)
        let line = Float(max(1, scale.rounded()))
        frame.append(
            [solid(SIMD4(cell.x, cell.y + cell.w - 2 * line, width, line), config.foreground)],
            texture: atlas.texture)
    }

    private func makeTexture(_ image: CGImage) -> MTLTexture? {
        GlyphRasterizer.image(image, maxDimension: maxTextureDimension).flatMap(makeTexture)
    }

    private func makeTexture(_ raster: RasterizedGlyph) -> MTLTexture? {
        guard raster.width > 0, raster.height > 0,
              raster.width <= maxTextureDimension, raster.height <= maxTextureDimension else { return nil }
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: raster.width, height: raster.height, mipmapped: false)
        descriptor.usage = .shaderRead
        descriptor.storageMode = device.hasUnifiedMemory ? .shared : .managed
        guard let texture = device.makeTexture(descriptor: descriptor) else { return nil }
        raster.pixels.withUnsafeBytes { bytes in
            texture.replace(
                region: MTLRegionMake2D(0, 0, raster.width, raster.height),
                mipmapLevel: 0, withBytes: bytes.baseAddress!, bytesPerRow: raster.width * 4)
        }
        return texture
    }

    // MARK: - Geometry

    private var cellWidth: CGFloat { CGFloat(config?.cellWidth ?? 8) }
    private var cellHeight: CGFloat { CGFloat(config?.cellHeight ?? 16) }

    private func pixel(_ points: CGFloat) -> CGFloat {
        (points * scale).rounded()
    }

    /// A rect in points snapped to whole pixels: x, y, width, height.
    private func pixelRect(_ rect: CGRect) -> SIMD4<Float> {
        let x0 = pixel(rect.minX)
        let y0 = pixel(rect.minY)
        return SIMD4(Float(x0), Float(y0), Float(pixel(rect.maxX) - x0), Float(pixel(rect.maxY) - y0))
    }

    private func cellPixels(row: Int, col: Int, width: Int = 1) -> SIMD4<Float> {
        let paddingX = CGFloat(config?.paddingX ?? 0)
        let paddingY = CGFloat(config?.paddingY ?? 0)
        return pixelRect(CGRect(
            x: paddingX + CGFloat(col) * cellWidth, y: paddingY + CGFloat(row) * cellHeight,
            width: CGFloat(width) * cellWidth, height: cellHeight))
    }

    private func rgba(_ color: TermyColor, alpha: Float = 1) -> SIMD4<Float> {
        SIMD4(Float(color.r) / 255, Float(color.g) / 255, Float(color.b) / 255, Float(color.a) / 255 * alpha)
    }

    private func solid(_ rect: SIMD4<Float>, _ color: TermyColor, alpha: Float = 1) -> MetalQuad {
        MetalQuad(rect: rect, color: rgba(color, alpha: alpha), mode: .solid)
    }
}
#endif
