import AppKit

/// What a second pane is showing.
public enum PaneKind: String, Sendable, CaseIterable {
    /// Quick Look, one file at a time.
    case preview
    /// A video or a sound, in AVKit's player.
    case player
    /// Sanitized HTML, in a web view that runs no script and loads nothing
    /// from the network unless it was told it may.
    case web
}

/// A key a pane may have a use for while it has the keyboard focus. Every
/// other key, and any of these with a modifier held, is left to the view
/// inside the pane.
public enum PaneKey: Equatable, Sendable, CaseIterable {
    case escape, space, left, right, up, down

    /// Key codes are those of the physical keys, which do not move with the
    /// keyboard layout.
    public init?(keyCode: UInt16, modifiers: NSEvent.ModifierFlags = []) {
        guard modifiers.isDisjoint(with: [.command, .control, .option, .shift]) else { return nil }
        switch keyCode {
        case 53: self = .escape
        case 49: self = .space
        case 123: self = .left
        case 124: self = .right
        case 125: self = .down
        case 126: self = .up
        default: return nil
        }
    }

    public init?(event: NSEvent) {
        guard event.type == .keyDown else { return nil }
        self.init(keyCode: event.keyCode, modifiers: event.modifierFlags)
    }
}

/// The second pane: a header with a title and a close button, over one
/// native viewer. `PreviewPane`, `PlayerPane` and `WebPane` are the viewers.
///
/// A pane knows a file and what to call it. It does not know what a message
/// is, and it never closes itself: it asks through `onClose`.
@MainActor
public class PaneView: NSView {
    public let kind: PaneKind
    /// What the header calls the thing being shown.
    public private(set) var title: String
    /// What the header says after the title: "2 of 5" for the second of five
    /// files, and nothing when there is nothing to add.
    public private(set) var detail = ""
    /// Shown in place of the viewer when there is nothing it can show.
    public private(set) var message: String?
    /// The close button was used.
    public var onClose: (() -> Void)?

    private let content: NSView
    private let titleLabel = NSTextField(labelWithString: "")
    private let detailLabel = NSTextField(labelWithString: "")
    private let messageLabel = NSTextField(wrappingLabelWithString: "")
    private let closeButton = NSButton()

    init(kind: PaneKind, title: String, content: NSView) {
        self.kind = kind
        self.title = title
        self.content = content
        super.init(frame: .zero)

        titleLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .medium)
        titleLabel.textColor = .secondaryLabelColor
        titleLabel.lineBreakMode = .byTruncatingMiddle
        titleLabel.maximumNumberOfLines = 1
        titleLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        titleLabel.stringValue = title

        detailLabel.font = .monospacedDigitSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        detailLabel.textColor = .tertiaryLabelColor
        detailLabel.maximumNumberOfLines = 1
        detailLabel.isHidden = true

        closeButton.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close pane")
        closeButton.isBordered = false
        closeButton.toolTip = "Close Pane (esc)"
        closeButton.target = self
        closeButton.action = #selector(close(_:))
        closeButton.setAccessibilityIdentifier("pane.close")

        let header = NSStackView(views: [titleLabel, detailLabel, NSView(), closeButton])
        header.orientation = .horizontal
        header.edgeInsets = NSEdgeInsets(top: 4, left: 10, bottom: 4, right: 8)

        messageLabel.alignment = .center
        messageLabel.textColor = .secondaryLabelColor
        messageLabel.isHidden = true

        let body = NSView()
        for view in [content, messageLabel] {
            view.translatesAutoresizingMaskIntoConstraints = false
            body.addSubview(view)
        }

        let stack = NSStackView(views: [header, body])
        stack.orientation = .vertical
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            header.widthAnchor.constraint(equalTo: stack.widthAnchor),
            body.widthAnchor.constraint(equalTo: stack.widthAnchor),
            content.topAnchor.constraint(equalTo: body.topAnchor),
            content.leadingAnchor.constraint(equalTo: body.leadingAnchor),
            content.trailingAnchor.constraint(equalTo: body.trailingAnchor),
            content.bottomAnchor.constraint(equalTo: body.bottomAnchor),
            messageLabel.centerXAnchor.constraint(equalTo: body.centerXAnchor),
            messageLabel.centerYAnchor.constraint(equalTo: body.centerYAnchor),
            messageLabel.widthAnchor.constraint(lessThanOrEqualTo: body.widthAnchor, constant: -40),
        ])
        // The viewer takes whatever height the header leaves.
        content.setContentHuggingPriority(.defaultLow, for: .vertical)
        body.setContentHuggingPriority(.defaultLow, for: .vertical)
        setAccessibilityIdentifier("pane.\(kind.rawValue)")
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    /// The pane itself takes the keyboard when its viewer cannot, so that
    /// `esc` still closes it.
    override public var acceptsFirstResponder: Bool { true }

    /// The view to give the keyboard focus to.
    public var focusTarget: NSView { content.isHidden ? self : content }

    /// A key pressed while the pane has the focus. True when the pane used
    /// it; false leaves it to the viewer inside. `esc` is not offered here:
    /// closing is the window's business, whatever the pane.
    public func handleKey(_: PaneKey) -> Bool { false }

    /// The pane's state in a few words, for a scripted run's dump.
    public var summary: String { message.map { "says \"\($0)\"" } ?? "" }

    /// Stops whatever the viewer is doing. Called once, when the pane is
    /// taken out of the window; the pane is not used again.
    public func tearDown() {}

    func setHeader(title: String, detail: String) {
        self.title = title
        self.detail = detail
        titleLabel.stringValue = title
        detailLabel.stringValue = detail
        detailLabel.isHidden = detail.isEmpty
    }

    /// Shows words in place of the viewer, or the viewer again for `nil`.
    func setMessage(_ text: String?) {
        message = text
        messageLabel.stringValue = text ?? ""
        messageLabel.isHidden = text == nil
        content.isHidden = text != nil
    }

    @objc private func close(_: Any?) {
        onClose?()
    }
}

/// Whether there is a file, and not a folder, at this address.
func isFile(_ url: URL) -> Bool {
    var isDirectory: ObjCBool = false
    return url.isFileURL && FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory)
        && !isDirectory.boolValue
}
