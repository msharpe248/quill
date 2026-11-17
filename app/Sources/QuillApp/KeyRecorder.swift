import AppKit
import Bridge

/// A field that takes a key by having it pressed: while it has the
/// keyboard, a key that is pressed is written down in place of doing
/// whatever it does.
///
/// It records and shows, and decides nothing: what was pressed goes to
/// whoever owns it as Quill writes keys (`KeyNotation.written`), and
/// whether the key can be had is the core's to say.
///
/// Three keys are not taken, because they are how a sheet is worked from
/// the keyboard: return presses its default button, esc its cancel
/// button, and tab moves on. The owner is told of the first two.
@MainActor
final class KeyRecorder: NSControl {
    /// A key was pressed: as Quill writes it.
    var onPress: ((String) -> Void)?
    /// Return was pressed, or esc: the sheet's to act on.
    var onReturn: (() -> Void)?
    var onEscape: (() -> Void)?

    /// What has been recorded, as Quill writes keys. Empty for nothing.
    private var recorded = ""

    private static let height: CGFloat = 24
    private static let inset: CGFloat = 7

    override var stringValue: String {
        get { recorded }
        set {
            recorded = newValue
            needsDisplay = true
            setAccessibilityValue(shown)
        }
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: NSView.noIntrinsicMetric, height: Self.height)
    }

    override var acceptsFirstResponder: Bool { isEnabled }
    override var canBecomeKeyView: Bool { isEnabled }
    override var focusRingMaskBounds: NSRect { bounds }

    override func drawFocusRingMask() {
        NSBezierPath(roundedRect: bounds, xRadius: 5, yRadius: 5).fill()
    }

    override func becomeFirstResponder() -> Bool {
        needsDisplay = true
        return super.becomeFirstResponder()
    }

    override func resignFirstResponder() -> Bool {
        needsDisplay = true
        return super.resignFirstResponder()
    }

    /// What the field says: the keys as Quill writes them, which is how
    /// the list beside it and the file have them, or what to do.
    private var shown: String {
        if recorded.isEmpty {
            return isRecording ? "Press a key\u{2026}" : "Click, then press a key"
        }
        return recorded
    }

    private var isRecording: Bool { window?.firstResponder === self }

    override func draw(_: NSRect) {
        let frame = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: frame, xRadius: 5, yRadius: 5)
        NSColor.controlBackgroundColor.setFill()
        path.fill()
        NSColor.separatorColor.setStroke()
        path.stroke()
        let color: NSColor =
            recorded.isEmpty ? .placeholderTextColor : (isEnabled ? .labelColor : .disabledControlTextColor)
        let text = NSAttributedString(
            string: shown,
            attributes: [.font: NSFont.systemFont(ofSize: NSFont.systemFontSize), .foregroundColor: color])
        let size = text.size()
        text.draw(at: NSPoint(x: Self.inset, y: ((bounds.height - size.height) / 2).rounded()))
    }

    override func mouseDown(with _: NSEvent) {
        guard isEnabled else { return }
        window?.makeFirstResponder(self)
    }

    /// The modifiers of a key press, as Quill names them.
    private static func modifiers(of event: NSEvent) -> KeyNotation.Modifiers {
        var held: KeyNotation.Modifiers = []
        let flags = event.modifierFlags
        if flags.contains(.control) { held.insert(.control) }
        if flags.contains(.option) { held.insert(.option) }
        if flags.contains(.shift) { held.insert(.shift) }
        if flags.contains(.command) { held.insert(.command) }
        return held
    }

    /// Takes a key press. Returns whether it was the field's to take.
    private func take(_ event: NSEvent) -> Bool {
        guard isEnabled, let characters = event.charactersIgnoringModifiers,
            let key = KeyNotation.written(characters: characters, modifiers: Self.modifiers(of: event))
        else { return false }
        switch key {
        case "enter":
            onReturn?()
        case "esc":
            onEscape?()
        case "tab":
            window?.selectNextKeyView(nil)
        case "shift-tab":
            window?.selectPreviousKeyView(nil)
        default:
            onPress?(key)
        }
        return true
    }

    override func keyDown(with event: NSEvent) {
        if !take(event) {
            super.keyDown(with: event)
        }
    }

    /// A key with cmd held goes to the menus before it is ever a key
    /// press: while this field has the keyboard, it is the field's.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard isRecording, event.type == .keyDown, event.modifierFlags.contains(.command) else {
            return super.performKeyEquivalent(with: event)
        }
        return take(event)
    }
}
