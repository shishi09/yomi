import Foundation
import AppKit
import ImageIO
import CoreGraphics

struct SomBubble: Codable {
    let id: Int
    let x: Double
    let y: Double
    let w: Double
    let h: Double
}

func fail(_ message: String) -> Never {
    fputs("ERROR: \(message)\n", stderr)
    exit(1)
}

/// Applique la transformation EXIF pour obtenir une image orientée vers le haut.
func createOrientedCGImage(from source: CGImageSource) -> CGImage? {
    guard let cgImage = CGImageSourceCreateImageAtIndex(source, 0, nil) else { return nil }
    var orientation: UInt32 = 1
    if let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
       let raw = props[kCGImagePropertyOrientation] as? UInt32 {
        orientation = raw
    }
    if orientation == 1 { return cgImage }

    let origW = cgImage.width
    let origH = cgImage.height
    let swapDims = (orientation >= 5 && orientation <= 8)
    let outW = swapDims ? origH : origW
    let outH = swapDims ? origW : origH

    guard let cs = cgImage.colorSpace ?? CGColorSpace(name: CGColorSpace.sRGB),
          let ctx = CGContext(
              data: nil,
              width: outW,
              height: outH,
              bitsPerComponent: 8,
              bytesPerRow: 0,
              space: cs,
              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
          ) else {
        return cgImage
    }

    var transform = CGAffineTransform.identity
    switch orientation {
    case 2:
        transform = CGAffineTransform(translationX: CGFloat(outW), y: 0).scaledBy(x: -1, y: 1)
    case 3:
        transform = CGAffineTransform(translationX: CGFloat(outW), y: CGFloat(outH)).rotated(by: .pi)
    case 4:
        transform = CGAffineTransform(translationX: 0, y: CGFloat(outH)).scaledBy(x: 1, y: -1)
    case 5:
        transform = CGAffineTransform(translationX: CGFloat(outW), y: CGFloat(outH)).rotated(by: -.pi / 2).scaledBy(x: -1, y: 1)
    case 6:
        transform = CGAffineTransform(translationX: 0, y: CGFloat(outH)).rotated(by: -.pi / 2)
    case 7:
        transform = CGAffineTransform(scaleX: -1, y: 1).rotated(by: -.pi / 2)
    case 8:
        transform = CGAffineTransform(translationX: CGFloat(outW), y: 0).rotated(by: .pi / 2)
    default:
        break
    }

    ctx.concatenate(transform)
    ctx.draw(cgImage, in: CGRect(x: 0, y: 0, width: origW, height: origH))
    return ctx.makeImage() ?? cgImage
}

let args = CommandLine.arguments
guard args.count >= 4 else {
    fail("Usage: annotate_som <input_image_path> <bubbles_json_path> <output_jpg_path> [max_dim=896]")
}

let inputPath = args[1]
let bubblesPath = args[2]
let outputPath = args[3]
let maxDim = args.count >= 5 ? max(256, Int(args[4]) ?? 896) : 896

let inputURL = URL(fileURLWithPath: inputPath)
guard let source = CGImageSourceCreateWithURL(inputURL as CFURL, nil),
      let cgImage = createOrientedCGImage(from: source) else {
    fail("Impossible d'ouvrir l'image: \(inputPath)")
}

let bubblesData = try Data(contentsOf: URL(fileURLWithPath: bubblesPath))
let bubbles = try JSONDecoder().decode([SomBubble].self, from: bubblesData)

let origW = cgImage.width
let origH = cgImage.height
let scale = min(1.0, Double(maxDim) / Double(max(origW, origH)))
let canvasW = max(1, Int((Double(origW) * scale).rounded()))
let canvasH = max(1, Int((Double(origH) * scale).rounded()))

guard let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: canvasW,
    pixelsHigh: canvasH,
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
) else {
    fail("Impossible de créer le bitmap de sortie.")
}

guard let nsCtx = NSGraphicsContext(bitmapImageRep: rep) else {
    fail("Impossible de créer le contexte graphique.")
}

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = nsCtx

let cgCtx = nsCtx.cgContext
cgCtx.interpolationQuality = .high
// Le système de coordonnées AppKit/CG a l'origine en bas à gauche :
// 1. Dessine l'image droite sur tout le canvas
cgCtx.draw(cgImage, in: CGRect(x: 0, y: 0, width: canvasW, height: canvasH))

let strokeColor = NSColor(red: 0.92, green: 0.12, blue: 0.12, alpha: 0.85)
let badgeBgColor = NSColor(red: 0.88, green: 0.08, blue: 0.08, alpha: 0.95)
let fontSize = CGFloat(max(10.0, min(14.0, Double(canvasW) * 0.015)))
let font = NSFont.boldSystemFont(ofSize: fontSize)
let textAttrs: [NSAttributedString.Key: Any] = [
    .font: font,
    .foregroundColor: NSColor.white,
]

for b in bubbles {
    let bx = CGFloat(b.x) * CGFloat(canvasW)
    let bw = max(4.0, CGFloat(b.w) * CGFloat(canvasW))
    let bh = max(4.0, CGFloat(b.h) * CGFloat(canvasH))
    // Conversion coordonnée haut-gauche -> bas-gauche
    let by = CGFloat(canvasH) - (CGFloat(b.y) * CGFloat(canvasH)) - bh

    let boxRect = NSRect(x: bx, y: by, width: bw, height: bh)
    let rectPath = NSBezierPath(rect: boxRect)
    rectPath.lineWidth = 2.0
    strokeColor.setStroke()
    rectPath.stroke()

    let label = "\(b.id)" as NSString
    let textSize = label.size(withAttributes: textAttrs)
    let padX: CGFloat = 3.5
    let padY: CGFloat = 1.0
    let badgeW = max(textSize.width + padX * 2, textSize.height + padY)
    let badgeH = textSize.height + padY * 2

    // Place le badge juste AU-DESSUS du coin haut-gauche (hors de la bulle pour ne jamais masquer le texte)
    let rawBadgeX = bx
    let rawBadgeY = by + bh + 1.0
    let badgeX = max(1.0, min(CGFloat(canvasW) - badgeW - 1.0, rawBadgeX))
    let badgeY = max(1.0, min(CGFloat(canvasH) - badgeH - 1.0, rawBadgeY))

    let badgeRect = NSRect(x: badgeX, y: badgeY, width: badgeW, height: badgeH)
    let badgePath = NSBezierPath(roundedRect: badgeRect, xRadius: 4.0, yRadius: 4.0)
    badgeBgColor.setFill()
    badgePath.fill()

    let tx = badgeX + (badgeW - textSize.width) / 2.0
    let ty = badgeY + (badgeH - textSize.height) / 2.0
    label.draw(at: NSPoint(x: tx, y: ty), withAttributes: textAttrs)
}

NSGraphicsContext.restoreGraphicsState()

guard let jpgData = rep.representation(using: .jpeg, properties: [.compressionFactor: 0.85]) else {
    fail("Impossible d'encoder le JPEG annoté.")
}

try jpgData.write(to: URL(fileURLWithPath: outputPath))
print("OK \(canvasW)x\(canvasH)")
