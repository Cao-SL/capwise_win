#!/usr/bin/env swift
// Draws the Capwise DMG background at 1x and 2x:
//   release-dmg-background.swift <out@1x.png> <out@2x.png>
// The window is 660×400 points; release.sh puts the app at (170, 178) and
// Applications at (490, 178), measured from the top left.
import AppKit

let size = NSSize(width: 660, height: 400)
let arguments = CommandLine.arguments
guard arguments.count == 3 else {
    FileHandle.standardError.write("usage: release-dmg-background.swift <1x.png> <2x.png>\n".data(using: .utf8)!)
    exit(2)
}

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> NSColor {
    NSColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255,
        alpha: alpha
    )
}

func centered(_ text: NSAttributedString, top: CGFloat) {
    let bounds = text.size()
    text.draw(at: NSPoint(x: (size.width - bounds.width) / 2, y: size.height - top - bounds.height))
}

func render(scale: CGFloat, to path: String) {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: Int(size.width * scale),
        pixelsHigh: Int(size.height * scale),
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 0,
        bitsPerPixel: 0
    )!
    rep.size = size
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

    color(0xF6F7F9).setFill()
    NSRect(origin: .zero, size: size).fill()

    let gradient = NSGradient(colors: [color(0xFFFFFF, 0.9), color(0xFFFFFF, 0)])!
    gradient.draw(
        in: NSBezierPath(ovalIn: NSRect(x: 80, y: 120, width: 500, height: 260)),
        relativeCenterPosition: .zero
    )

    let arrowY = size.height - 178
    let arrow = NSBezierPath()
    arrow.move(to: NSPoint(x: 284, y: arrowY))
    arrow.line(to: NSPoint(x: 370, y: arrowY))
    arrow.lineWidth = 3
    arrow.lineCapStyle = .round
    color(0xC4CAD4).setStroke()
    arrow.stroke()
    let head = NSBezierPath()
    head.move(to: NSPoint(x: 364, y: arrowY + 9))
    head.line(to: NSPoint(x: 377, y: arrowY))
    head.line(to: NSPoint(x: 364, y: arrowY - 9))
    head.lineWidth = 3
    head.lineCapStyle = .round
    head.lineJoinStyle = .round
    head.stroke()

    let muted = color(0x8A93A3)
    let strong = color(0x1D2129)
    let primary = NSFont.systemFont(ofSize: 15, weight: .regular)
    let primaryBold = NSFont.systemFont(ofSize: 15, weight: .semibold)
    let line = NSMutableAttributedString()
    for (text, bold) in [("将 ", false), ("灵录", true), (" 拖到 ", false), ("应用程序", true), (" 文件夹", false)] {
        line.append(NSAttributedString(string: text, attributes: [
            .font: bold ? primaryBold : primary,
            .foregroundColor: bold ? strong : muted,
        ]))
    }
    centered(line, top: 288)
    centered(NSAttributedString(string: "Drag Capwise to Applications · Requires macOS 13.3 or later", attributes: [
        .font: NSFont.systemFont(ofSize: 11.5, weight: .regular),
        .foregroundColor: color(0xA3AAB6),
    ]), top: 318)

    NSGraphicsContext.restoreGraphicsState()
    guard let png = rep.representation(using: .png, properties: [:]) else {
        FileHandle.standardError.write("could not encode \(path)\n".data(using: .utf8)!)
        exit(1)
    }
    try! png.write(to: URL(fileURLWithPath: path))
}

render(scale: 1, to: arguments[1])
render(scale: 2, to: arguments[2])
