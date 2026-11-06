// Rasterizes an SVG to a square PNG with AppKit's native SVG support.
// Usage: swift render.swift <in.svg> <out.png> <size> [<size> <out.png>]...
import AppKit

let args = Array(CommandLine.arguments.dropFirst())
guard args.count >= 3, args.count % 2 == 1 else {
    FileHandle.standardError.write("usage: render.swift <in.svg> <out.png> <size> [<out.png> <size>]...\n".data(using: .utf8)!)
    exit(2)
}
guard let image = NSImage(contentsOfFile: args[0]) else {
    FileHandle.standardError.write("cannot load \(args[0])\n".data(using: .utf8)!)
    exit(1)
}

for pair in stride(from: 1, to: args.count, by: 2) {
    let out = args[pair]
    guard let side = Int(args[pair + 1]), side > 0 else {
        FileHandle.standardError.write("bad size \(args[pair + 1])\n".data(using: .utf8)!)
        exit(2)
    }
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: side, pixelsHigh: side,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    rep.size = NSSize(width: side, height: side)
    NSGraphicsContext.saveGraphicsState()
    let context = NSGraphicsContext(bitmapImageRep: rep)!
    context.imageInterpolation = .high
    NSGraphicsContext.current = context
    image.draw(in: NSRect(x: 0, y: 0, width: side, height: side))
    NSGraphicsContext.restoreGraphicsState()
    guard let png = rep.representation(using: .png, properties: [:]) else {
        FileHandle.standardError.write("cannot encode \(out)\n".data(using: .utf8)!)
        exit(1)
    }
    try png.write(to: URL(fileURLWithPath: out))
}
