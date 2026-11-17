import AVFoundation
import AVKit
import AppKit
import Foundation
import Testing

@testable import Panes

@MainActor
@Suite(.serialized)
struct PlayerPaneTests {
    /// A silent film of one colour: small frames, a few to the second, no
    /// sound track at all.
    private func writeMovie(to file: URL, seconds: Int) async throws {
        let side = 64
        let framesPerSecond: Int32 = 5
        let writer = try AVAssetWriter(outputURL: file, fileType: .mov)
        let input = AVAssetWriterInput(
            mediaType: .video,
            outputSettings: [
                AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: side, AVVideoHeightKey: side,
            ])
        input.expectsMediaDataInRealTime = false
        let adaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: input,
            sourcePixelBufferAttributes: [
                kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
                kCVPixelBufferWidthKey as String: side,
                kCVPixelBufferHeightKey as String: side,
            ])
        writer.add(input)
        guard writer.startWriting() else {
            throw writer.error ?? CocoaError(.fileWriteUnknown)
        }
        writer.startSession(atSourceTime: .zero)
        let frames = Int64(seconds) * Int64(framesPerSecond)
        for frame in 0..<frames {
            while !input.isReadyForMoreMediaData {
                try await Task.sleep(for: .milliseconds(5))
            }
            var made: CVPixelBuffer?
            CVPixelBufferCreate(nil, side, side, kCVPixelFormatType_32BGRA, nil, &made)
            let buffer = try #require(made)
            CVPixelBufferLockBaseAddress(buffer, [])
            memset(CVPixelBufferGetBaseAddress(buffer), 0x60, CVPixelBufferGetDataSize(buffer))
            CVPixelBufferUnlockBaseAddress(buffer, [])
            guard adaptor.append(buffer, withPresentationTime: CMTime(value: frame, timescale: framesPerSecond))
            else {
                throw writer.error ?? CocoaError(.fileWriteUnknown)
            }
        }
        input.markAsFinished()
        writer.endSession(atSourceTime: CMTime(value: frames, timescale: framesPerSecond))
        await withCheckedContinuation { done in
            writer.finishWriting { done.resume() }
        }
        guard writer.status == .completed else {
            throw writer.error ?? CocoaError(.fileWriteUnknown)
        }
    }

    /// One second of silence as a WAV file: the 44 bytes of header and
    /// nothing but zeros after.
    private func writeSilence(to file: URL) throws {
        let rate: UInt32 = 8000
        let samples = Data(count: Int(rate) * 2)
        var data = Data()
        func append<T: FixedWidthInteger>(_ value: T) {
            withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) }
        }
        data.append(contentsOf: Array("RIFF".utf8))
        append(UInt32(36 + samples.count))
        data.append(contentsOf: Array("WAVEfmt ".utf8))
        append(UInt32(16))
        append(UInt16(1))  // PCM
        append(UInt16(1))  // one channel
        append(rate)
        append(rate * 2)  // bytes a second
        append(UInt16(2))  // bytes a sample
        append(UInt16(16))  // bits a sample
        data.append(contentsOf: Array("data".utf8))
        append(UInt32(samples.count))
        data.append(samples)
        try data.write(to: file)
    }

    private func seconds(_ pane: PlayerPane) -> Double {
        pane.player.currentTime().seconds
    }

    private func waitForTime(_ wanted: Double, in pane: PlayerPane) async throws {
        try await wait(for: "the player to be at \(wanted)s, not \(seconds(pane))s") {
            abs(seconds(pane) - wanted) < 0.25
        }
    }

    @Test func aMovieOpensPausedAndTheKeysPlayPauseAndSeek() async throws {
        let scratch = try Scratch()
        let movie = scratch.file("clip.mov")
        try await writeMovie(to: movie, seconds: 12)

        let pane = PlayerPane(file: movie, title: "Clip")
        defer { pane.tearDown() }
        // The film has no sound, and would not be heard if it had.
        pane.player.isMuted = true
        #expect(pane.kind == .player)
        #expect(pane.title == "Clip")
        #expect(pane.summary == "loading")
        try await wait(for: "the movie to be ready") { pane.isReady || pane.player.currentItem?.status == .failed }
        #expect(pane.player.currentItem?.status == .readyToPlay)

        // Nothing plays until it is asked to.
        #expect(pane.player.rate == 0)
        #expect(seconds(pane) == 0)
        #expect(pane.summary == "paused at 0:00 of 0:12")

        // The arrows move five seconds, and stop at both ends.
        #expect(pane.handleKey(.right))
        try await waitForTime(5, in: pane)
        #expect(pane.handleKey(.right))
        try await waitForTime(10, in: pane)
        #expect(pane.handleKey(.right))
        try await waitForTime(12, in: pane)
        #expect(pane.handleKey(.left))
        try await waitForTime(7, in: pane)
        #expect(pane.summary == "paused at 0:07 of 0:12")
        #expect(pane.handleKey(.left))
        #expect(pane.handleKey(.left))
        try await waitForTime(0, in: pane)
        // Two presses one straight after the other are two steps.
        #expect(pane.handleKey(.right))
        #expect(pane.handleKey(.right))
        try await waitForTime(10, in: pane)
        #expect(pane.handleKey(.left))
        try await waitForTime(5, in: pane)
        #expect(pane.player.rate == 0)

        // Space plays, and the film moves; space again pauses it.
        #expect(pane.handleKey(.space))
        #expect(pane.player.rate == 1)
        #expect(pane.isPlaying)
        try await wait(for: "the film to move on") { seconds(pane) > 5.2 }
        #expect(pane.summary.hasPrefix("playing at 0:0"))
        #expect(pane.handleKey(.space))
        #expect(pane.player.rate == 0)
        // The film comes to rest, and then stays where it is. Its clock may
        // run on for an instant after the pause is asked for (it did on a
        // virtual Mac), so it is let settle before it is held to standing
        // still. One that never stopped would never settle.
        try await wait(for: "the film to come to rest") {
            let before = seconds(pane)
            try? await Task.sleep(for: .milliseconds(60))
            return seconds(pane) == before
        }
        let stopped = seconds(pane)
        let still = try await holds(for: 0.3) { seconds(pane) == stopped }
        #expect(still)

        // Not the player's keys.
        #expect(!pane.handleKey(.up))
        #expect(!pane.handleKey(.down))
        #expect(!pane.handleKey(.escape))

        pane.tearDown()
        #expect(pane.player.currentItem == nil)
        #expect(pane.player.rate == 0)
    }

    @Test func spaceAtTheEndStartsAgainFromTheBeginning() async throws {
        let scratch = try Scratch()
        let movie = scratch.file("short.mov")
        try await writeMovie(to: movie, seconds: 1)
        let pane = PlayerPane(file: movie, title: "Short")
        defer { pane.tearDown() }
        pane.player.isMuted = true
        try await wait(for: "the movie to be ready") { pane.isReady }
        #expect(pane.handleKey(.right))
        try await waitForTime(1, in: pane)
        #expect(pane.handleKey(.space))
        #expect(pane.isPlaying)
        try await wait(for: "the film to start again") { seconds(pane) < 0.9 }
        // Left to itself it plays to the end and stops there.
        try await wait(for: "the film to end") { !pane.isPlaying }
        #expect(abs(seconds(pane) - 1) < 0.25)
    }

    @Test func aSoundIsReadyAndSilentUntilAsked() async throws {
        let scratch = try Scratch()
        let sound = scratch.file("silence.wav")
        try writeSilence(to: sound)
        let pane = PlayerPane(file: sound, title: "Silence")
        defer { pane.tearDown() }
        pane.player.isMuted = true
        try await wait(for: "the sound to be ready") { pane.isReady || pane.player.currentItem?.status == .failed }
        #expect(pane.player.currentItem?.status == .readyToPlay)
        #expect(pane.player.rate == 0)
        #expect(pane.summary == "paused at 0:00 of 0:01")
        let silent = try await holds(for: 0.3) { pane.player.rate == 0 && seconds(pane) == 0 }
        #expect(silent)
    }

    @Test func whatCannotBePlayedSaysSo() async throws {
        let scratch = try Scratch()
        let missing = PlayerPane(file: scratch.file("missing.mov"), title: "Missing")
        defer { missing.tearDown() }
        #expect(missing.message != nil)
        #expect(missing.player.currentItem == nil)
        // The keys are harmless with nothing to play.
        #expect(missing.handleKey(.space))
        #expect(missing.handleKey(.right))
        #expect(missing.player.rate == 0)
        #expect(missing.summary.hasPrefix("says "))

        let nonsense = try scratch.write("this is not a film", to: "nonsense.mov")
        let pane = PlayerPane(file: nonsense, title: "Nonsense")
        defer { pane.tearDown() }
        try await wait(for: "the player to give up") { pane.message != nil }
        #expect(pane.player.currentItem?.status == .failed)
        #expect(pane.handleKey(.right))
    }

    @Test func thePlayerTakesTheKeyboardAndOffersFullScreenAndPictureInPicture() throws {
        let scratch = try Scratch()
        let sound = scratch.file("silence.wav")
        try writeSilence(to: sound)
        let pane = PlayerPane(file: sound, title: "Silence")
        defer { pane.tearDown() }
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 300), styleMask: [.borderless],
            backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.contentView = pane
        #expect(!window.isVisible)
        #expect(window.makeFirstResponder(pane.focusTarget))
        let responder = try #require(window.firstResponder as? NSView)
        #expect(responder.isDescendant(of: pane))

        let view = try #require(pane.focusTarget as? AVPlayerView)
        #expect(view.player === pane.player)
        #expect(view.controlsStyle == .inline)
        #expect(view.showsFullScreenToggleButton)
        #expect(view.allowsPictureInPicturePlayback)
        window.contentView = nil
    }

    @Test func secondsAreWrittenAsAClock() {
        #expect(PlayerPane.clock(0) == "0:00")
        #expect(PlayerPane.clock(7.9) == "0:07")
        #expect(PlayerPane.clock(75) == "1:15")
        #expect(PlayerPane.clock(3600) == "60:00")
        #expect(PlayerPane.clock(.nan) == "0:00")
        #expect(PlayerPane.clock(-3) == "0:00")
    }
}
