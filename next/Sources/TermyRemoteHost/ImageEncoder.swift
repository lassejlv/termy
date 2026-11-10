import CoreGraphics
import Foundation
import ImageIO
import Termy
import TermyAppKit
import TermyRemoteKit

/// Sends a pane's Kitty graphics to a phone: each image's pixels once, as a
/// PNG no larger than a phone can show, and then where to draw them.
///
/// The phone keeps what it was sent. This side decides what it may forget
/// (least recently placed first, never an image on screen) and says so in the
/// next list, so both ends agree on what the phone holds.
struct ImageEncoder {
    /// The longest side of an image sent to a phone, and the smaller sizes
    /// tried when the PNG is still too large to send.
    static let maxDimensions = [1024, 512, 256]
    /// The images a phone keeps beyond those on screen, in encoded bytes.
    static let idleBytes = 8 * 1024 * 1024

    /// What the phone holds, least recently placed first.
    private var held: [(key: KittyImageKey, bytes: Int)] = []
    /// Images too large to send, so they aren't encoded for every list.
    private var skipped: Set<KittyImageKey> = []
    private var sentList = false

    /// The messages that bring the phone's images up to date with `terminal`.
    mutating func update(stream: Int, terminal: Terminal, configuration: RemoteConfiguration) -> [HostMessage] {
        let known = Set(held.map(\.key)).union(skipped)
        guard let snapshot = try? terminal.kittyGraphics(holding: known) else { return [] }
        let cell = CGSize(width: max(1, configuration.cellWidth), height: max(1, configuration.cellHeight))
        let pixelsPerPoint = CGSize(
            width: snapshot.cellWidth > 0 ? CGFloat(snapshot.cellWidth) / cell.width : 1,
            height: snapshot.cellHeight > 0 ? CGFloat(snapshot.cellHeight) / cell.height : 1)

        var messages: [HostMessage] = []
        var placed: Set<KittyImageKey> = []
        var placements: [RemoteImagePlacement] = []
        for placement in snapshot.placements.prefix(RemoteImages.maxPlacements) {
            let key = placement.imageKey
            if skipped.contains(key) { continue }
            if let index = held.firstIndex(where: { $0.key == key }) {
                if placed.insert(key).inserted { held.append(held.remove(at: index)) }
            } else {
                guard let png = Self.png(placement) else {
                    skipped.insert(key)
                    continue
                }
                held.append((key, png.count))
                placed.insert(key)
                messages.append(.image(stream: stream, key: Self.name(key), png: png))
            }
            let geometry = KittyPlacementGeometry(
                placement, cellWidth: cell.width, cellHeight: cell.height, pixelsPerPoint: pixelsPerPoint)
            guard !geometry.visible.isNull, placement.imageWidth > 0, placement.imageHeight > 0 else { continue }
            let crop = placement.sourceCrop
            let width = Double(placement.imageWidth)
            let height = Double(placement.imageHeight)
            placements.append(RemoteImagePlacement(
                key: Self.name(key),
                frame: Self.cells(geometry.image, cell),
                clip: Self.cells(geometry.clip, cell),
                crop: [crop.minX / width, crop.minY / height, crop.width / width, crop.height / height],
                z: Int(placement.zIndex)))
        }
        skipped.formIntersection(Set(snapshot.placements.map(\.imageKey)))

        var evicted: [String] = []
        var idle = held.filter { !placed.contains($0.key) }.reduce(0) { $0 + $1.bytes }
        while idle > Self.idleBytes, let index = held.firstIndex(where: { !placed.contains($0.key) }) {
            let entry = held.remove(at: index)
            idle -= entry.bytes
            evicted.append(Self.name(entry.key))
        }
        messages.append(.images(RemoteImages(stream: stream, placements: placements, evicted: evicted, full: !sentList)))
        sentList = true
        return messages
    }

    private static func name(_ key: KittyImageKey) -> String { "\(key.imageID):\(key.generation)" }

    private static func cells(_ rect: CGRect, _ cell: CGSize) -> [Double] {
        [rect.minX / cell.width, rect.minY / cell.height, rect.width / cell.width, rect.height / cell.height]
    }

    /// The placement's image as a PNG small enough to send, or nil.
    private static func png(_ placement: KittyGraphicsPlacement) -> Data? {
        guard let image = decode(placement) else { return nil }
        for dimension in maxDimensions {
            if let png = png(image, maxDimension: dimension), png.count <= RemoteImages.maxImageBytes { return png }
        }
        return nil
    }

    private static func png(_ image: CGImage, maxDimension: Int) -> Data? {
        let longest = max(image.width, image.height)
        guard longest > maxDimension else { return encode(image) }
        let scale = Double(maxDimension) / Double(longest)
        let width = max(1, Int((Double(image.width) * scale).rounded()))
        let height = max(1, Int((Double(image.height) * scale).rounded()))
        guard let context = CGContext(
            data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return nil }
        context.interpolationQuality = .high
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return context.makeImage().flatMap(encode)
    }

    private static func decode(_ placement: KittyGraphicsPlacement) -> CGImage? {
        let (width, height) = (placement.imageWidth, placement.imageHeight)
        if let rgba = placement.rgba {
            guard width > 0, height > 0, rgba.count == width * height * 4,
                  let provider = CGDataProvider(data: rgba as CFData) else { return nil }
            return CGImage(
                width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
                space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
                provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
        }
        guard !placement.png.isEmpty, let provider = CGDataProvider(data: placement.png as CFData) else { return nil }
        return CGImage(pngDataProviderSource: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    private static func encode(_ image: CGImage) -> Data? {
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(data, "public.png" as CFString, 1, nil) else {
            return nil
        }
        CGImageDestinationAddImage(destination, image, nil)
        return CGImageDestinationFinalize(destination) ? data as Data : nil
    }
}
