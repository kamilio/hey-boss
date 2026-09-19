import AppKit
import Foundation

// Reuse Hey Boss's existing SF Symbol identity. No new artwork is authored.
let folder = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
for (size, name, badge) in [(512, "icon-512.png", false), (192, "icon-192.png", false), (180, "apple-touch-icon.png", false), (96, "badge-96.png", true)] {
    guard let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
          let context = NSGraphicsContext(bitmapImageRep: bitmap),
          let symbol = NSImage(systemSymbolName: "bubble.left.and.bubble.right.fill", accessibilityDescription: nil)?.withSymbolConfiguration(.init(pointSize: CGFloat(size) * 0.52, weight: .medium))?.withSymbolConfiguration(.init(paletteColors: [.white])) else {
        throw NSError(domain: "HeyBossIcons", code: 1)
    }
    NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = context
    if !badge {
        NSColor(srgbRed: 0.24, green: 0.29, blue: 0.72, alpha: 1).setFill()
        NSRect(x: 0, y: 0, width: size, height: size).fill()
    }
    let aspect = symbol.size.width / symbol.size.height
    let width = CGFloat(size) * (badge ? 0.84 : 0.59), height = width / aspect
    let rect = NSRect(x: (CGFloat(size) - width) / 2, y: (CGFloat(size) - height) / 2, width: width, height: height)
    symbol.draw(in: rect)
    NSGraphicsContext.restoreGraphicsState()
    guard let data = bitmap.representation(using: .png, properties: [:]) else { throw NSError(domain: "HeyBossIcons", code: 2) }
    try data.write(to: folder.appendingPathComponent(name))
}
