import AppKit

/// What a second pane is showing.
public enum PaneKind: String, Sendable, CaseIterable {
    /// Quick Look, one file at a time.
    case preview
    /// A video or a sound, in AVKit's player.
    case player
    /// Sanitized HTML, in a web view that runs no script and loads nothing
    /// from the network unless it was told it may: the reading pane.
    case web
    /// A terminal of its own, running the Vim that ships in the app on a
    /// message being written.
    case editor
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

/// Something a pane says about what it shows, in a bar of its own between
/// its header and the viewer, with a button for each thing that can be done
/// about it. The pane gives the words and the buttons no meaning: a button
/// has a name, and using it reports the name.
public struct PaneNotice: Equatable, Sendable {
    public struct Button: Equatable, Sendable {
        /// What `onNoticeButton` is given when the button is used.
        public var name: String
        /// What the button says.
        public var title: String

        public init(name: String, title: String) {
            self.name = name
            self.title = title
        }
    }

    public var text: String
    public var buttons: [Button]

    public init(text: String, buttons: [Button] = []) {
        self.text = text
        self.buttons = buttons
    }
}

/// The second pane: a header with a title and a close button, over one
/// native viewer. `PreviewPane`, `PlayerPane` and `WebPane` are the viewers;
/// `EditorPane` holds a terminal instead.
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
    /// Lines of plain text under the title, in a pane that has them: the
    /// reading pane says who a message is from and to there, outside the
    /// page, where the page cannot reach.
    public let headerLines: [String]
    /// What the pane says about what it shows, in a bar under the header,
    /// in a pane that has something to say.
    public let notice: PaneNotice?
    /// A button of the notice was used: its name.
    public var onNoticeButton: ((String) -> Void)?
    /// Whether the pane has a close button. The reading pane has none: it
    /// goes when the core says so.
    public let isClosable: Bool
    /// Shown in place of the viewer when there is nothing it can show.
    public private(set) var message: String?
    /// The close button was used.
    public var onClose: (() -> Void)?

    private let content: NSView
    private let titleLabel = NSTextField(labelWithString: "")
    private let detailLabel = NSTextField(labelWithString: "")
    private let messageLabel = NSTextField(wrappingLabelWithString: "")
    private let closeButton = NSButton()

    /// `closeHint` is what the close button says of itself when pointed at.
    /// `headerLines`, when given, makes the header that of a pane to read
    /// in: the title set larger, and these lines under it. `closable`
    /// false leaves the close button out.
    init(
        kind: PaneKind, title: String, content: NSView, closeHint: String = "Close Pane (esc)",
        headerLines: [String]? = nil, notice: PaneNotice? = nil, closable: Bool = true
    ) {
        self.kind = kind
        self.title = title
        self.content = content
        self.headerLines = headerLines ?? []
        self.notice = notice
        isClosable = closable
        super.init(frame: .zero)

        let reading = headerLines != nil
        titleLabel.font =
            reading
            ? .systemFont(ofSize: NSFont.systemFontSize + 2, weight: .semibold)
            : .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .medium)
        titleLabel.textColor = reading ? .labelColor : .secondaryLabelColor
        titleLabel.lineBreakMode = reading ? .byTruncatingTail : .byTruncatingMiddle
        titleLabel.maximumNumberOfLines = 1
        titleLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        titleLabel.stringValue = title

        detailLabel.font = .monospacedDigitSystemFont(ofSize: NSFont.smallSystemFontSize, weight: .regular)
        detailLabel.textColor = .tertiaryLabelColor
        detailLabel.maximumNumberOfLines = 1
        detailLabel.isHidden = true

        closeButton.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close pane")
        closeButton.isBordered = false
        closeButton.toolTip = closeHint
        closeButton.target = self
        closeButton.action = #selector(close(_:))
        closeButton.setAccessibilityIdentifier("pane.close")
        closeButton.isHidden = !closable

        let top = NSStackView(views: [titleLabel, detailLabel, NSView(), closeButton])
        top.orientation = .horizontal
        // Under the title, each on a line of its own, cut to the pane's
        // width: plain text, whatever it says.
        let lines = self.headerLines.map { line in
            let label = NSTextField(labelWithString: line)
            label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            label.textColor = .secondaryLabelColor
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            label.allowsDefaultTighteningForTruncation = false
            label.isSelectable = true
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            return label
        }
        let insets =
            reading
            ? NSEdgeInsets(top: 10, left: 16, bottom: 10, right: 12)
            : NSEdgeInsets(top: 4, left: 10, bottom: 4, right: 8)
        let header = NSStackView(views: [top] + lines)
        header.orientation = .vertical
        header.alignment = .leading
        header.spacing = 2
        header.edgeInsets = insets
        if reading, !title.isEmpty {
            header.setCustomSpacing(6, after: top)
        }
        // A pane to read in with nothing to say above its page says
        // nothing: no empty bar.
        header.isHidden = reading && title.isEmpty && lines.isEmpty
        for view in [top] + lines {
            view.widthAnchor.constraint(lessThanOrEqualTo: header.widthAnchor, constant: -(insets.left + insets.right))
                .isActive = true
        }
        top.widthAnchor.constraint(equalTo: header.widthAnchor, constant: -(insets.left + insets.right))
            .isActive = true

        messageLabel.alignment = .center
        messageLabel.textColor = .secondaryLabelColor
        messageLabel.isHidden = true

        let body = NSView()
        for view in [content, messageLabel] {
            view.translatesAutoresizingMaskIntoConstraints = false
            body.addSubview(view)
        }

        let bar = notice.map { makeNoticeBar($0, leading: insets.left, trailing: insets.right) }
        let stack = NSStackView(views: [header] + (bar.map { [$0] } ?? []) + [body])
        stack.orientation = .vertical
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        bar?.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
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

    /// The bar the notice is said in: the words at the left, cut short
    /// before a button is, and the buttons at the right. It is a shade
    /// apart from the header above it and the viewer below, in a light
    /// window and in a dark one.
    private func makeNoticeBar(_ notice: PaneNotice, leading: CGFloat, trailing: CGFloat) -> NSView {
        let label = NSTextField(labelWithString: notice.text)
        label.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        label.textColor = .labelColor
        label.lineBreakMode = .byTruncatingTail
        label.maximumNumberOfLines = 1
        label.toolTip = notice.text
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        label.setAccessibilityIdentifier("pane.notice.text")

        let buttons = notice.buttons.map { button in
            let control = NSButton(title: button.title, target: self, action: #selector(useNoticeButton(_:)))
            control.bezelStyle = .rounded
            control.controlSize = .small
            control.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            control.identifier = NSUserInterfaceItemIdentifier(button.name)
            // A button is never cut short, and never takes the keyboard
            // from the list: it is clicked, and the keys stay where they
            // were.
            control.refusesFirstResponder = true
            control.setContentCompressionResistancePriority(.required, for: .horizontal)
            control.setContentHuggingPriority(.required, for: .horizontal)
            control.setAccessibilityIdentifier("pane.notice.\(button.name)")
            return control
        }

        let row = NSStackView(views: [label, NSView()] + buttons)
        row.orientation = .horizontal
        row.spacing = 8
        row.edgeInsets = NSEdgeInsets(top: 5, left: leading, bottom: 5, right: trailing)
        row.translatesAutoresizingMaskIntoConstraints = false

        let bar = NSBox()
        bar.boxType = .custom
        bar.titlePosition = .noTitle
        bar.borderWidth = 0
        bar.cornerRadius = 0
        bar.contentViewMargins = .zero
        bar.fillColor = .unemphasizedSelectedContentBackgroundColor
        bar.addSubview(row)
        NSLayoutConstraint.activate([
            row.topAnchor.constraint(equalTo: bar.topAnchor),
            row.bottomAnchor.constraint(equalTo: bar.bottomAnchor),
            row.leadingAnchor.constraint(equalTo: bar.leadingAnchor),
            row.trailingAnchor.constraint(equalTo: bar.trailingAnchor),
        ])
        bar.setContentHuggingPriority(.required, for: .vertical)
        bar.setAccessibilityIdentifier("pane.notice")
        return bar
    }

    @objc private func useNoticeButton(_ sender: NSButton) {
        if let name = sender.identifier?.rawValue {
            pressNoticeButton(name)
        }
    }

    /// Uses the notice's button of this name, as a click on it does. False
    /// when the pane has no such button.
    @discardableResult
    public func pressNoticeButton(_ name: String) -> Bool {
        guard notice?.buttons.contains(where: { $0.name == name }) == true else { return false }
        onNoticeButton?(name)
        return true
    }

    /// The notice in a few words, for a scripted run's dump.
    var noticeSummary: String? {
        notice.map { notice in
            (["notice \(notice.text.debugDescription)"] + notice.buttons.map { "[\($0.title)]" })
                .joined(separator: " ")
        }
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

    /// Whether every key pressed while the pane has the focus belongs to
    /// what is inside it, `esc` included: the window then takes none of
    /// them, neither to close the pane nor to offer to `handleKey`.
    public var takesEveryKey: Bool { false }

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
