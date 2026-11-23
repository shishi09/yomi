import Foundation
import Vision
import ImageIO
import CoreGraphics

struct OCRLine: Codable {
    let text: String
    let confidence: Float
    let x: Double
    let y: Double
    let w: Double
    let h: Double
    let white_ratio: Double
    let dark_ratio: Double
    let bubble_region_id: Int
    let region_x: Double?
    let region_y: Double?
    let region_w: Double?
    let region_h: Double?
}

struct OCRResult: Codable {
    let width: Int
    let height: Int
    let lines: [OCRLine]
}

struct RawLine {
    let text: String
    let confidence: Float
    let x: Double
    let y: Double
    let w: Double
    let h: Double
    let whiteRatio: Double
    let darkRatio: Double
}

struct RegionInfo {
    let id: Int
    let rx: Double
    let ry: Double
    let rw: Double
    let rh: Double
}

func fail(_ message: String) -> Never {
    fputs("ERROR: \(message)\n", stderr)
    exit(1)
}

/// Rend l'image en niveaux de gris (taille réduite) pour analyser rapidement
/// le fond autour de chaque bulle et détecter les contours fermés.
func buildGrayscaleBuffer(from cgImage: CGImage, maxDim: Int = 900) -> (pixels: [UInt8], w: Int, h: Int)? {
    let origW = cgImage.width
    let origH = cgImage.height
    guard origW > 0, origH > 0 else { return nil }

    let scale = min(1.0, Double(maxDim) / Double(max(origW, origH)))
    let w = max(1, Int(Double(origW) * scale))
    let h = max(1, Int(Double(origH) * scale))

    var pixels = [UInt8](repeating: 255, count: w * h)
    let colorSpace = CGColorSpaceCreateDeviceGray()
    guard let ctx = CGContext(
        data: &pixels,
        width: w,
        height: h,
        bitsPerComponent: 8,
        bytesPerRow: w,
        space: colorSpace,
        bitmapInfo: CGImageAlphaInfo.none.rawValue
    ) else {
        return nil
    }
    ctx.interpolationQuality = .low
    ctx.draw(cgImage, in: CGRect(x: 0, y: 0, width: w, height: h))
    return (pixels, w, h)
}

func sampleBackgroundStats(
    buffer: (pixels: [UInt8], w: Int, h: Int)?,
    x: Double, y: Double, w: Double, h: Double
) -> (whiteRatio: Double, darkRatio: Double) {
    guard let buf = buffer else { return (1.0, 0.0) }
    let bw = buf.w
    let bh = buf.h

    let padX = max(0.004, min(0.014, h * 0.35))
    let padY = max(0.003, min(0.012, h * 0.30))

    let ox1 = max(0, Int((x - padX) * Double(bw)))
    let oy1 = max(0, Int((y - padY) * Double(bh)))
    let ox2 = min(bw - 1, Int((x + w + padX) * Double(bw)))
    let oy2 = min(bh - 1, Int((y + h + padY) * Double(bh)))

    let ix1 = max(0, Int(x * Double(bw)))
    let iy1 = max(0, Int(y * Double(bh)))
    let ix2 = min(bw - 1, Int((x + w) * Double(bw)))
    let iy2 = min(bh - 1, Int((y + h) * Double(bh)))

    var total = 0
    var whiteCount = 0
    var darkCount = 0

    let stepX = max(1, (ox2 - ox1) / 24)
    let stepY = max(1, (oy2 - oy1) / 16)

    var py = oy1
    while py <= oy2 {
        var px = ox1
        while px <= ox2 {
            let insideInner = (px >= ix1 && px <= ix2 && py >= iy1 && py <= iy2)
            if !insideInner {
                let luma = buf.pixels[py * bw + px]
                total += 1
                if luma >= 218 {
                    whiteCount += 1
                } else if luma <= 55 {
                    darkCount += 1
                }
            }
            px += stepX
        }
        py += stepY
    }

    if total == 0 {
        return (1.0, 0.0)
    }
    return (Double(whiteCount) / Double(total), Double(darkCount) / Double(total))
}

/// Détecte les bulles blanches fermées par flood-fill (BFS) à partir des lignes OCR.
/// Retourne pour chaque ligne son identifiant de bulle fermée (nil si bulle ouverte / hors bulle)
/// ainsi que le rectangle intérieur de la bulle (queue de bulle rognée).
func detectClosedBubbleRegions(
    buffer: (pixels: [UInt8], w: Int, h: Int)?,
    rawLines: [RawLine]
) -> [RegionInfo?] {
    guard let buf = buffer, !rawLines.isEmpty else {
        return Array(repeating: nil, count: rawLines.count)
    }

    let bw = buf.w
    let bh = buf.h
    let totalPixels = bw * bh

    var passable = [Bool](repeating: false, count: totalPixels)
    for i in 0..<totalPixels {
        if buf.pixels[i] >= 198 {
            passable[i] = true
        }
    }

    // Marque le cœur interne des lignes OCR sur fond blanc comme traversable
    // afin que les lettres noires ne fragmentent pas l'intérieur de la bulle,
    // tout en laissant intact le bord noir extérieur de la bulle.
    for ln in rawLines {
        if ln.whiteRatio < 0.50 || (ln.h >= 0.045 && ln.whiteRatio < 0.68) { continue }
        let shrinkX = max(0.002, ln.w * 0.08)
        let shrinkY = max(0.002, ln.h * 0.14)
        let x1 = max(1, Int((ln.x + shrinkX) * Double(bw)))
        let y1 = max(1, Int((ln.y + shrinkY) * Double(bh)))
        let x2 = min(bw - 2, Int((ln.x + ln.w - shrinkX) * Double(bw)))
        let y2 = min(bh - 2, Int((ln.y + ln.h - shrinkY) * Double(bh)))
        if x2 <= x1 || y2 <= y1 { continue }
        for py in y1...y2 {
            let rowOff = py * bw
            for px in x1...x2 {
                passable[rowOff + px] = true
            }
        }
    }

    // regionMap[idx] : 0 = non visité, -2 = zone ouverte (marge/fond), >0 = id de bulle fermée
    var regionMap = [Int](repeating: 0, count: totalPixels)
    var regionInfos: [Int: RegionInfo] = [:]
    var nextRegionId = 1

    let maxClosedPixels = Int(Double(totalPixels) * 0.17)
    let maxClosedW = Int(Double(bw) * 0.52)
    let maxClosedH = Int(Double(bh) * 0.44)

    func findSeed(for ln: RawLine) -> Int? {
        let cx = max(0, min(bw - 1, Int((ln.x + ln.w * 0.5) * Double(bw))))
        let cy = max(0, min(bh - 1, Int((ln.y + ln.h * 0.5) * Double(bh))))
        if passable[cy * bw + cx] { return cy * bw + cx }

        let x1 = max(0, Int(ln.x * Double(bw)))
        let y1 = max(0, Int(ln.y * Double(bh)))
        let x2 = min(bw - 1, Int((ln.x + ln.w) * Double(bw)))
        let y2 = min(bh - 1, Int((ln.y + ln.h) * Double(bh)))
        let stepX = max(1, (x2 - x1) / 8)
        let stepY = max(1, (y2 - y1) / 4)
        var py = y1
        while py <= y2 {
            var px = x1
            while px <= x2 {
                let idx = py * bw + px
                if passable[idx] { return idx }
                px += stepX
            }
            py += stepY
        }
        return nil
    }

    func floodFill(seed: Int) -> Int {
        if regionMap[seed] != 0 {
            return regionMap[seed]
        }

        var queue: [Int] = [seed]
        var visited: [Int] = [seed]
        var head = 0
        regionMap[seed] = -1

        var minX = seed % bw
        var maxX = minX
        var minY = seed / bw
        var maxY = minY
        var touchesPageBorder = false
        var overflowed = false

        while head < queue.count {
            let cur = queue[head]
            head += 1

            let px = cur % bw
            let py = cur / bw
            if px < minX { minX = px }
            if px > maxX { maxX = px }
            if py < minY { minY = py }
            if py > maxY { maxY = py }

            if px == 0 || py == 0 || px == bw - 1 || py == bh - 1 {
                touchesPageBorder = true
            }

            if visited.count > maxClosedPixels || (maxX - minX) > maxClosedW || (maxY - minY) > maxClosedH {
                overflowed = true
                break
            }

            if px > 0 {
                let nb = cur - 1
                if passable[nb] && regionMap[nb] == 0 {
                    regionMap[nb] = -1
                    queue.append(nb)
                    visited.append(nb)
                }
            }
            if px + 1 < bw {
                let nb = cur + 1
                if passable[nb] && regionMap[nb] == 0 {
                    regionMap[nb] = -1
                    queue.append(nb)
                    visited.append(nb)
                }
            }
            if py > 0 {
                let nb = cur - bw
                if passable[nb] && regionMap[nb] == 0 {
                    regionMap[nb] = -1
                    queue.append(nb)
                    visited.append(nb)
                }
            }
            if py + 1 < bh {
                let nb = cur + bw
                if passable[nb] && regionMap[nb] == 0 {
                    regionMap[nb] = -1
                    queue.append(nb)
                    visited.append(nb)
                }
            }
        }

        if overflowed || touchesPageBorder || visited.count < 40 {
            for idx in visited {
                regionMap[idx] = -2
            }
            return -2
        }

        let rid = nextRegionId
        nextRegionId += 1

        let spanH = maxY - minY + 1
        let spanW = maxX - minX + 1
        var rowCounts = [Int](repeating: 0, count: spanH)
        var colCounts = [Int](repeating: 0, count: spanW)

        for idx in visited {
            regionMap[idx] = rid
            let px = idx % bw
            let py = idx / bw
            rowCounts[py - minY] += 1
            colCounts[px - minX] += 1
        }

        let maxRow = rowCounts.max() ?? 1
        let maxCol = colCounts.max() ?? 1
        let rowThresh = max(2, Int(Double(maxRow) * 0.30))
        let colThresh = max(2, Int(Double(maxCol) * 0.30))

        var trimMinY = minY
        for r in 0..<spanH {
            if rowCounts[r] >= rowThresh { trimMinY = minY + r; break }
        }
        var trimMaxY = maxY
        for r in stride(from: spanH - 1, through: 0, by: -1) {
            if rowCounts[r] >= rowThresh { trimMaxY = minY + r; break }
        }
        var trimMinX = minX
        for c in 0..<spanW {
            if colCounts[c] >= colThresh { trimMinX = minX + c; break }
        }
        var trimMaxX = maxX
        for c in stride(from: spanW - 1, through: 0, by: -1) {
            if colCounts[c] >= colThresh { trimMaxX = minX + c; break }
        }

        let insetX = Double(trimMaxX - trimMinX) * 0.05
        let insetY = Double(trimMaxY - trimMinY) * 0.05
        let rx = max(0.0, (Double(trimMinX) + insetX) / Double(bw))
        let ry = max(0.0, (Double(trimMinY) + insetY) / Double(bh))
        let rw = max(0.01, (Double(trimMaxX - trimMinX) - 2.0 * insetX) / Double(bw))
        let rh = max(0.01, (Double(trimMaxY - trimMinY) - 2.0 * insetY) / Double(bh))

        regionInfos[rid] = RegionInfo(id: rid, rx: rx, ry: ry, rw: rw, rh: rh)
        return rid
    }

    var results: [RegionInfo?] = []
    results.reserveCapacity(rawLines.count)

    for ln in rawLines {
        if ln.whiteRatio < 0.50 || (ln.h >= 0.045 && ln.whiteRatio < 0.68) {
            results.append(nil)
            continue
        }
        guard let seed = findSeed(for: ln) else {
            results.append(nil)
            continue
        }
        let rid = floodFill(seed: seed)
        if rid > 0, let info = regionInfos[rid] {
            results.append(info)
        } else {
            results.append(nil)
        }
    }
    return results
}

let args = CommandLine.arguments
guard args.count >= 2 else {
    fail("Usage: vision_ocr <image_path> [en|ja|auto]")
}

let imagePath = args[1]
let langMode = args.count >= 3 ? args[2].lowercased() : "en"

let imageURL = URL(fileURLWithPath: imagePath)
guard let source = CGImageSourceCreateWithURL(imageURL as CFURL, nil) else {
    fail("Impossible d'ouvrir l'image: \(imagePath)")
}

guard let cgImage = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
    fail("Impossible de décoder l'image: \(imagePath)")
}

var orientation: CGImagePropertyOrientation = .up
if let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
   let rawOrientation = props[kCGImagePropertyOrientation] as? UInt32,
   let parsed = CGImagePropertyOrientation(rawValue: rawOrientation) {
    orientation = parsed
}

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true

switch langMode {
case "ja":
    request.recognitionLanguages = ["ja-JP", "en-US"]
case "auto":
    if #available(macOS 13.0, *) {
        request.automaticallyDetectsLanguage = true
    } else {
        request.recognitionLanguages = ["en-US", "ja-JP"]
    }
default:
    request.recognitionLanguages = ["en-US"]
}

let handler = VNImageRequestHandler(cgImage: cgImage, orientation: orientation, options: [:])
do {
    try handler.perform([request])
} catch {
    fail("Erreur Apple Vision OCR: \(error.localizedDescription)")
}

let grayBuffer = buildGrayscaleBuffer(from: cgImage)
let observations = request.results ?? []
var rawLines: [RawLine] = []

for obs in observations {
    guard let candidate = obs.topCandidates(1).first else { continue }
    let text = candidate.string.trimmingCharacters(in: .whitespacesAndNewlines)
    if text.isEmpty { continue }

    let box = obs.boundingBox
    let x = max(0.0, min(1.0, Double(box.origin.x)))
    let y = max(0.0, min(1.0, Double(1.0 - box.origin.y - box.size.height)))
    let w = max(0.0, min(1.0, Double(box.size.width)))
    let h = max(0.0, min(1.0, Double(box.size.height)))

    let bg = sampleBackgroundStats(buffer: grayBuffer, x: x, y: y, w: w, h: h)
    rawLines.append(RawLine(
        text: text,
        confidence: candidate.confidence,
        x: x, y: y, w: w, h: h,
        whiteRatio: (bg.whiteRatio * 1000).rounded() / 1000,
        darkRatio: (bg.darkRatio * 1000).rounded() / 1000
    ))
}

let regions = detectClosedBubbleRegions(buffer: grayBuffer, rawLines: rawLines)

var lines: [OCRLine] = []
for (idx, ln) in rawLines.enumerated() {
    let reg = regions[idx]
    lines.append(OCRLine(
        text: ln.text,
        confidence: ln.confidence,
        x: ln.x,
        y: ln.y,
        w: ln.w,
        h: ln.h,
        white_ratio: ln.whiteRatio,
        dark_ratio: ln.darkRatio,
        bubble_region_id: reg?.id ?? -1,
        region_x: reg.map { ($0.rx * 100000).rounded() / 100000 },
        region_y: reg.map { ($0.ry * 100000).rounded() / 100000 },
        region_w: reg.map { ($0.rw * 100000).rounded() / 100000 },
        region_h: reg.map { ($0.rh * 100000).rounded() / 100000 }
    ))
}

let result = OCRResult(
    width: cgImage.width,
    height: cgImage.height,
    lines: lines
)

let encoder = JSONEncoder()
do {
    let data = try encoder.encode(result)
    if let json = String(data: data, encoding: .utf8) {
        print(json)
    }
} catch {
    fail("Erreur encodage JSON: \(error.localizedDescription)")
}
