import AVKit
import AppKit

/// A video or a sound in the second pane, in AVKit's player with its usual
/// controls. `space` plays and pauses; the left and right arrows move five
/// seconds back and forwards.
///
/// Nothing plays until the user asks: the pane opens paused on the first
/// frame, so a file never makes a sound by being looked at.
@MainActor
public final class PlayerPane: PaneView {
    /// How far an arrow key moves, in seconds.
    public static let seekStep: Double = 5

    public let player: AVPlayer
    private let playerView = AVPlayerView()
    private var statusObservation: NSKeyValueObservation?
    /// Where a seek that has not finished is going, so that a second press
    /// of an arrow moves on from there and not from where the picture still
    /// is.
    private var seekTarget: CMTime?

    public init(file: URL, title: String) {
        player = AVPlayer()
        super.init(kind: .player, title: title, content: playerView)

        player.actionAtItemEnd = .pause
        playerView.controlsStyle = .inline
        playerView.showsFullScreenToggleButton = true
        playerView.allowsPictureInPicturePlayback = true
        playerView.player = player

        guard isFile(file) else {
            setMessage("This file is no longer there to play.")
            return
        }
        let item = AVPlayerItem(url: file)
        statusObservation = item.observe(\.status) { [weak self] item, _ in
            guard item.status == .failed else { return }
            Task { @MainActor in
                self?.setMessage("This file cannot be played.")
            }
        }
        player.replaceCurrentItem(with: item)
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    /// Whether the file has been read far enough to play.
    public var isReady: Bool { player.currentItem?.status == .readyToPlay }

    public var isPlaying: Bool { player.rate != 0 }

    override public func handleKey(_ key: PaneKey) -> Bool {
        switch key {
        case .space: togglePlaying()
        case .left: seek(by: -Self.seekStep)
        case .right: seek(by: Self.seekStep)
        case .up, .down, .escape: return false
        }
        return true
    }

    private func togglePlaying() {
        guard player.currentItem != nil else { return }
        if isPlaying {
            player.pause()
            return
        }
        // Played to the end: start again, as the play button does.
        if let duration = duration, player.currentTime().seconds >= duration {
            player.seek(to: .zero)
        }
        player.play()
    }

    /// The length in seconds, once it is known and if it has one.
    private var duration: Double? {
        guard isReady, let seconds = player.currentItem?.duration.seconds, seconds.isFinite else { return nil }
        return seconds
    }

    private func seek(by seconds: Double) {
        guard isReady else { return }
        var wanted = (seekTarget ?? player.currentTime()).seconds + seconds
        if let duration {
            wanted = min(wanted, duration)
        }
        let target = CMTime(seconds: max(wanted, 0), preferredTimescale: 600)
        seekTarget = target
        player.seek(to: target, toleranceBefore: .zero, toleranceAfter: .zero) { [weak self] _ in
            Task { @MainActor in
                guard let self, self.seekTarget == target else { return }
                self.seekTarget = nil
            }
        }
    }

    override public var summary: String {
        guard message == nil else { return super.summary }
        let status =
            switch player.currentItem?.status {
            case .readyToPlay: isPlaying ? "playing" : "paused"
            case .failed: "failed"
            default: "loading"
            }
        var words = [status]
        if isReady {
            words.append("at \(Self.clock(player.currentTime().seconds))")
            if let duration {
                words.append("of \(Self.clock(duration))")
            }
        }
        return words.joined(separator: " ")
    }

    /// Seconds as minutes and seconds: 75 is "1:15".
    static func clock(_ seconds: Double) -> String {
        guard seconds.isFinite, seconds >= 0 else { return "0:00" }
        let whole = Int(seconds.rounded(.down))
        let tail = String(whole % 60)
        return "\(whole / 60):\(tail.count < 2 ? "0" : "")\(tail)"
    }

    override public func tearDown() {
        statusObservation = nil
        player.pause()
        player.replaceCurrentItem(with: nil)
        playerView.player = nil
    }
}
