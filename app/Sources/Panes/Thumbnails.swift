import AppKit
import ImageIO
import QuickLookThumbnailing
import UniformTypeIdentifiers

/// Pictures of files for the core to draw in the terminal: a picture it
/// cannot decode itself, the first page of a PDF, the poster frame of a
/// video. Made by Quick Look, which knows every kind of file the Mac does.
public enum Thumbnails {
    /// Makes a PNG of `file`, its longer side `size` pixels, and writes it
    /// to `out`. True when the PNG is there.
    ///
    /// `out` must be inside `folder`, the app's temporary folder: the path
    /// comes from the core, and the shell writes nowhere else on its word.
    /// Runs off the main thread.
    public static func make(file: URL, out: URL, size: Int, in folder: URL = TemporaryFolder.url) async -> Bool {
        guard size > 0, TemporaryFolder.contains(out, in: folder), isFile(file) else { return false }
        let request = QLThumbnailGenerator.Request(
            fileAt: file,
            size: CGSize(width: size, height: size),
            scale: 1,
            // The file's own picture, not its type's icon: an icon is not
            // what the message holds.
            representationTypes: .thumbnail)
        guard let thumbnail = try? await QLThumbnailGenerator.shared.generateBestRepresentation(for: request),
            let png = png(of: thumbnail.cgImage)
        else { return false }
        do {
            // Written whole or not at all, and in place of whatever is at
            // the path, never through it.
            try png.write(to: out, options: .atomic)
            return true
        } catch {
            return false
        }
    }

    static func png(of image: CGImage) -> Data? {
        let data = NSMutableData()
        guard
            let destination = CGImageDestinationCreateWithData(
                data as CFMutableData, UTType.png.identifier as CFString, 1, nil)
        else { return nil }
        CGImageDestinationAddImage(destination, image, nil)
        return CGImageDestinationFinalize(destination) ? data as Data : nil
    }
}
