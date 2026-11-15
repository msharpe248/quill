import AppKit

/// A terminal in the second pane, for the Vim that ships in the app.
///
/// The pane is the frame around the terminal and no more: it is handed the
/// terminal's view, and knows nothing of what runs in it. Every key is the
/// program's, `esc` most of all, so the pane uses none; closing it is asked
/// for with the close button or the menu, and whoever owns the program
/// decides what that means.
@MainActor
public final class EditorPane: PaneView {
    /// The pane has been asked to close, and the program in it to save and
    /// leave. It stays until the program has gone.
    public private(set) var isClosing = false

    public init(title: String, terminal: NSView) {
        super.init(kind: .editor, title: title, content: terminal, closeHint: "Save and Close")
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    override public var takesEveryKey: Bool { true }

    override public var summary: String { isClosing ? "closing" : "running" }

    /// Says in the header that the pane is on its way out.
    public func setClosing() {
        isClosing = true
        setHeader(title: title, detail: "saving and closing")
    }
}
