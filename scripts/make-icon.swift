// Draws the app icon (a "real copy" dot linked to a ring, the app's own status marks) and writes a 1024px PNG.
// Usage: swift scripts/make-icon.swift out.png
import AppKit

let size = 1024.0
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                           bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
func rgb(_ r: Double, _ g: Double, _ b: Double) -> NSColor { NSColor(red: r / 255, green: g / 255, blue: b / 255, alpha: 1) }

// macOS icon grid: an 824pt rounded square centred in the 1024 canvas.
let plate = NSBezierPath(roundedRect: NSRect(x: 100, y: 100, width: 824, height: 824), xRadius: 185, yRadius: 185)
NSGradient(starting: rgb(34, 39, 46), ending: rgb(15, 17, 20))!.draw(in: plate, angle: -90)

let (left, right, y) = (372.0, 652.0, 512.0)
let link = NSBezierPath()
link.move(to: NSPoint(x: left, y: y)); link.line(to: NSPoint(x: right - 118, y: y))
link.lineWidth = 26; rgb(70, 78, 88).setStroke(); link.stroke()

rgb(104, 200, 132).setFill()
NSBezierPath(ovalIn: NSRect(x: left - 118, y: y - 118, width: 236, height: 236)).fill()

let ring = NSBezierPath(ovalIn: NSRect(x: right - 100, y: y - 100, width: 200, height: 200))
ring.lineWidth = 40; rgb(96, 156, 245).setStroke(); ring.stroke()

try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
