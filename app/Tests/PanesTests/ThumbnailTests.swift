import AppKit
import CoreGraphics
import Foundation
import ImageIO
import Testing

@testable import Panes

struct ThumbnailTests {
    /// A picture drawn here: a coloured field with a block in one corner.
    private func writePNG(to file: URL, width: Int, height: Int) throws {
        let context = try #require(
            CGContext(
                data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        context.setFillColor(CGColor(red: 0.2, green: 0.4, blue: 0.8, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.setFillColor(CGColor(red: 0.9, green: 0.7, blue: 0.1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: width / 3, height: height / 3))
        let image = try #require(context.makeImage())
        let data = try #require(Thumbnails.png(of: image))
        try data.write(to: file)
    }

    /// A PDF of one upright page with a few shapes on it.
    private func writePDF(to file: URL) throws {
        var box = CGRect(x: 0, y: 0, width: 300, height: 400)
        let context = try #require(CGContext(file as CFURL, mediaBox: &box, nil))
        context.beginPDFPage(nil)
        context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        context.fill(box)
        context.setFillColor(CGColor(red: 0.8, green: 0.1, blue: 0.1, alpha: 1))
        context.fill(CGRect(x: 40, y: 40, width: 220, height: 120))
        context.endPDFPage()
        context.closePDF()
    }

    private func size(ofPNG file: URL) throws -> (width: Int, height: Int) {
        let data = try Data(contentsOf: file)
        #expect(data.prefix(8) == Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]))
        let source = try #require(CGImageSourceCreateWithData(data as CFData, nil))
        let image = try #require(CGImageSourceCreateImageAtIndex(source, 0, nil))
        return (image.width, image.height)
    }

    @Test func aPictureIsPicturedAtTheSizeAsked() async throws {
        let scratch = try Scratch()
        let picture = scratch.file("picture.png")
        try writePNG(to: picture, width: 400, height: 200)
        let out = scratch.file("thumb-1.png")

        #expect(await Thumbnails.make(file: picture, out: out, size: 64))
        let size = try size(ofPNG: out)
        // The longer side is the size asked for; the shape is kept.
        #expect((60...64).contains(size.width), "\(size)")
        #expect((28...34).contains(size.height), "\(size)")
    }

    @Test func theFirstPageOfAPDFIsPictured() async throws {
        let scratch = try Scratch()
        let document = scratch.file("document.pdf")
        try writePDF(to: document)
        let out = scratch.file("thumb-2.png")

        #expect(await Thumbnails.make(file: document, out: out, size: 128))
        let size = try size(ofPNG: out)
        #expect((120...128).contains(size.height), "\(size)")
        #expect((88...100).contains(size.width), "\(size)")
    }

    @Test func aPictureAlreadyThereIsReplaced() async throws {
        let scratch = try Scratch()
        let picture = scratch.file("picture.png")
        try writePNG(to: picture, width: 200, height: 200)
        let out = try scratch.write("not a picture", to: "thumb-3.png")
        #expect(await Thumbnails.make(file: picture, out: out, size: 32))
        let size = try size(ofPNG: out)
        #expect(max(size.width, size.height) <= 32)
    }

    @Test func aFileThatIsNotThereAnswersFailure() async throws {
        let scratch = try Scratch()
        let out = scratch.file("thumb-4.png")
        #expect(await Thumbnails.make(file: scratch.file("missing.pdf"), out: out, size: 64) == false)
        #expect(await Thumbnails.make(file: scratch.url, out: out, size: 64) == false)
        #expect(!FileManager.default.fileExists(atPath: out.path))
    }

    @Test func nothingIsWrittenOutsideTheTemporaryFolder() async throws {
        // Two folders side by side: one stands for the app's temporary
        // folder, the other for anywhere else.
        let inside = try Scratch()
        let elsewhere = try Scratch()
        let picture = inside.file("picture.png")
        try writePNG(to: picture, width: 200, height: 100)

        let out = elsewhere.file("thumb-5.png")
        #expect(await Thumbnails.make(file: picture, out: out, size: 64, in: inside.url) == false)
        #expect(!FileManager.default.fileExists(atPath: out.path))

        // Nor through a link that leads out of it.
        try FileManager.default.createSymbolicLink(at: inside.file("way-out"), withDestinationURL: elsewhere.url)
        let sly = inside.file("way-out/thumb-6.png")
        #expect(await Thumbnails.make(file: picture, out: sly, size: 64, in: inside.url) == false)
        #expect(try FileManager.default.contentsOfDirectory(atPath: elsewhere.url.path) == [])

        // The same request with the folder it is in is honoured.
        #expect(await Thumbnails.make(file: picture, out: inside.file("thumb-7.png"), size: 64, in: inside.url))
        // And the real temporary folder is what is asked about by default.
        let home = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("quill-thumbnail-test.png")
        #expect(await Thumbnails.make(file: picture, out: home, size: 64) == false)
        #expect(!FileManager.default.fileExists(atPath: home.path))
    }

    @Test func aLinkWhereThePictureGoesIsReplacedNotFollowed() async throws {
        let inside = try Scratch()
        let elsewhere = try Scratch()
        let picture = inside.file("picture.png")
        try writePNG(to: picture, width: 200, height: 100)
        // A link that does not lead anywhere yet, pointing out of the folder.
        let target = elsewhere.file("planted.png")
        let out = inside.file("thumb-8.png")
        try FileManager.default.createSymbolicLink(at: out, withDestinationURL: target)

        #expect(await Thumbnails.make(file: picture, out: out, size: 64, in: inside.url))
        #expect(!FileManager.default.fileExists(atPath: target.path))
        let kind = try FileManager.default.attributesOfItem(atPath: out.path)[.type] as? FileAttributeType
        #expect(kind == .typeRegular)
    }
}
