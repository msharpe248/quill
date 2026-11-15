import AppKit
import QuickLookUI

/// The second pane: a Quick Look preview of one file, with a title and a
/// close button, shown beside the terminal.
@MainActor
final class PreviewPane: NSView {
    private let titleLabel = NSTextField(labelWithString: "")
    private let closeButton = NSButton()
    private let preview: QLPreviewView
    var onClose: (() -> Void)?

    override init(frame: NSRect) {
        // `.normal` always returns a view; the initialiser is failable only
        // for Objective-C's sake.
        preview = QLPreviewView(frame: .zero, style: .normal) ?? QLPreviewView()
        super.init(frame: frame)

        titleLabel.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .medium)
        titleLabel.textColor = .secondaryLabelColor
        titleLabel.lineBreakMode = .byTruncatingMiddle

        closeButton.image = NSImage(systemSymbolName: "xmark", accessibilityDescription: "Close preview")
        closeButton.isBordered = false
        closeButton.toolTip = "Close Preview (esc)"
        closeButton.target = self
        closeButton.action = #selector(close(_:))
        closeButton.setAccessibilityIdentifier("preview.close")

        let header = NSStackView(views: [titleLabel, NSView(), closeButton])
        header.orientation = .horizontal
        header.edgeInsets = NSEdgeInsets(top: 4, left: 10, bottom: 4, right: 8)

        let stack = NSStackView(views: [header, preview])
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
            preview.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
        setAccessibilityIdentifier("preview.pane")
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    /// The file being shown, for tests and for deciding where focus is.
    private(set) var fileURL: URL?

    func show(_ url: URL, title: String) {
        fileURL = url
        titleLabel.stringValue = title
        preview.previewItem = url as NSURL
    }

    /// The view to give keyboard focus to.
    var focusTarget: NSView { preview }

    @objc private func close(_: Any?) {
        onClose?()
    }
}
