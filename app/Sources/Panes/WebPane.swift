import AppKit
import Bridge
import WebKit

/// What to do with a navigation the web view asks leave for.
public enum WebNavigationDecision: Equatable, Sendable {
    /// The pane's own document: let it load.
    case allow
    /// A link the user clicked: hand it to the browser or mail program, and
    /// leave the pane where it is.
    case open(URL)
    /// Anything else: nothing happens.
    case refuse
}

/// The one rule for where the web view may go: nowhere.
///
/// It may load the document it was made for, and move about inside it. A
/// click on a web or mail address is handed over to be opened elsewhere.
/// Everything else is refused: other files, other schemes, redirects and
/// refreshes nobody clicked, forms, frames, downloads and new windows.
public struct WebNavigationPolicy: Equatable, Sendable {
    /// Where a navigation would happen.
    public enum Target: Equatable, Sendable {
        case mainFrame, subframe, newWindow
    }

    /// The HTML file the pane shows.
    public let document: URL

    public init(document: URL) {
        self.document = document
    }

    public func decide(url: URL?, clicked: Bool, target: Target, download: Bool = false) -> WebNavigationDecision {
        guard let url else { return .refuse }
        if target == .mainFrame, !download, isDocument(url) {
            return .allow
        }
        // Only what the user clicked is opened, and only what `Links` lets
        // the rest of the shell open. A page that sends itself somewhere
        // must not be able to start the browser.
        if clicked, !download, let address = Links.openable(url.absoluteString) {
            return .open(address)
        }
        return .refuse
    }

    /// The document itself, at any place inside it (`#section`).
    func isDocument(_ url: URL) -> Bool {
        guard url.isFileURL, url.query == nil else { return false }
        return url.resolvingSymlinksInPath().standardizedFileURL.path
            == document.resolvingSymlinksInPath().standardizedFileURL.path
    }
}

/// The rules that keep the web view off the network, compiled by WebKit and
/// enforced in its own processes, below anything a page can reach.
@MainActor
public enum ContentRules {
    /// Nothing but the files beside the document and what is written into
    /// the document itself (`data:`). The first rule blocks every load of
    /// every kind; the others give back only those two.
    static let offline = """
        [
          {"trigger": {"url-filter": ".*"}, "action": {"type": "block"}},
          {"trigger": {"url-filter": "^file:"}, "action": {"type": "ignore-previous-rules"}},
          {"trigger": {"url-filter": "^data:"}, "action": {"type": "ignore-previous-rules"}}
        ]
        """

    /// The user allowed remote content: pictures, styles, fonts and media
    /// may also come over http and https. Still no scripts, frames, pages,
    /// pings, fetches, sockets or any other scheme.
    static let remote = """
        [
          {"trigger": {"url-filter": ".*"}, "action": {"type": "block"}},
          {"trigger": {"url-filter": "^file:"}, "action": {"type": "ignore-previous-rules"}},
          {"trigger": {"url-filter": "^data:"}, "action": {"type": "ignore-previous-rules"}},
          {
            "trigger": {
              "url-filter": "^https?:",
              "resource-type": ["image", "style-sheet", "font", "media", "svg-document"]
            },
            "action": {"type": "ignore-previous-rules"}
          }
        ]
        """

    private static var compiled: [Bool: WKContentRuleList] = [:]

    /// The compiled rules. They are compiled afresh in every run of the app,
    /// never looked up from an earlier one, so rules from an older version
    /// cannot be the ones in force.
    public static func list(allowingRemote: Bool) async throws -> WKContentRuleList {
        if let list = compiled[allowingRemote] {
            return list
        }
        let folder = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("quill-content-rules", isDirectory: true)
        guard let store = WKContentRuleListStore(url: folder) else {
            throw CocoaError(.fileWriteUnknown)
        }
        let list = try await store.compileContentRuleList(
            forIdentifier: allowingRemote ? "quill.remote" : "quill.offline",
            encodedContentRuleList: allowingRemote ? remote : offline)
        guard let list else {
            throw CocoaError(.fileReadCorruptFile)
        }
        compiled[allowingRemote] = list
        return list
    }
}

/// HTML in the second pane.
///
/// The core has already sanitized the file. This pane is the second line of
/// defence, and assumes the first one failed: no script runs, nothing is
/// kept between panes, nothing is loaded from the network unless the user
/// allowed remote content, and the view goes nowhere but its own document.
@MainActor
public final class WebPane: PaneView, WKNavigationDelegate, WKUIDelegate {
    /// How far the pane has got.
    public enum Phase: String, Sendable {
        /// The rules are being compiled. Nothing has been loaded.
        case preparing
        case loading
        case loaded
        /// The rules could not be put in place, or the file could not be
        /// shown. Nothing is on screen but a line saying so.
        case failed
    }

    public let policy: WebNavigationPolicy
    /// The user allowed this document to load pictures and styles from the
    /// network.
    public let allowsRemoteContent: Bool
    public private(set) var phase = Phase.preparing
    /// Addresses the view was refused leave to go to, the latest last.
    public private(set) var refused: [String] = []
    /// For tests. Nothing outside this module loads anything into it.
    let webView: WKWebView

    private let openLink: (URL) -> Void
    private var preparation: Task<Void, Never>?
    private var closed = false

    /// - Parameter openLink: Given a web or mail address the user clicked.
    public init(file: URL, title: String, allowsRemoteContent: Bool, openLink: @escaping (URL) -> Void) {
        policy = WebNavigationPolicy(document: file)
        self.allowsRemoteContent = allowsRemoteContent
        self.openLink = openLink
        webView = MessageWebView(frame: .zero, configuration: Self.configuration())
        super.init(kind: .web, title: title, content: webView)

        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.allowsBackForwardNavigationGestures = false
        // A link preview would load the page it points at.
        webView.allowsLinkPreview = false
        webView.allowsMagnification = true

        guard isFile(file) else {
            fail("This page is no longer there to show.")
            return
        }
        preparation = Task { [weak self] in
            await self?.prepareAndLoad()
        }
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    static func configuration() -> WKWebViewConfiguration {
        let configuration = WKWebViewConfiguration()
        // Cookies, caches and storage live in memory and die with the pane.
        configuration.websiteDataStore = .nonPersistent()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = false
        LegacyPreferences.turnOffJavaScript(in: configuration.preferences)
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.preferences.isElementFullscreenEnabled = false
        configuration.mediaTypesRequiringUserActionForPlayback = .all
        configuration.allowsAirPlayForMediaPlayback = false
        return configuration
    }

    /// Nothing may load before the rules are in force, so the load is
    /// started here, after them, and from nowhere else.
    private func prepareAndLoad() async {
        let rules: WKContentRuleList
        do {
            rules = try await ContentRules.list(allowingRemote: allowsRemoteContent)
        } catch {
            NSLog("Quill: the web view's rules could not be compiled: %@", String(describing: error))
            fail("This page cannot be shown safely.")
            return
        }
        guard !closed else { return }
        webView.configuration.userContentController.add(rules)
        phase = .loading
        // Read access to the document's own folder and nothing above it.
        webView.loadFileURL(policy.document, allowingReadAccessTo: policy.document.deletingLastPathComponent())
    }

    private func fail(_ text: String) {
        phase = .failed
        setMessage(text)
    }

    override public var summary: String {
        var words = ["remote content \(allowsRemoteContent ? "allowed" : "blocked")", phase.rawValue]
        if !refused.isEmpty {
            words.append("refused \(refused.count)")
        }
        if message != nil {
            words.append(super.summary)
        }
        return words.joined(separator: ", ")
    }

    override public func tearDown() {
        closed = true
        preparation?.cancel()
        webView.stopLoading()
        webView.navigationDelegate = nil
        webView.uiDelegate = nil
    }

    // MARK: - Where the view may go

    /// Applies the policy to one navigation: opens what is to be opened,
    /// notes what is refused, and tells WebKit whether to go on.
    func resolve(
        url: URL?, clicked: Bool, target: WebNavigationPolicy.Target, download: Bool = false
    ) -> WKNavigationActionPolicy {
        switch policy.decide(url: url, clicked: clicked, target: target, download: download) {
        case .allow:
            return .allow
        case let .open(address):
            openLink(address)
            return .cancel
        case .refuse:
            refused.append(url?.absoluteString ?? "")
            // A page that keeps trying must not grow this without end.
            refused.removeFirst(max(refused.count - 50, 0))
            return .cancel
        }
    }

    public func webView(
        _: WKWebView, decidePolicyFor action: WKNavigationAction, preferences: WKWebpagePreferences
    ) async -> (WKNavigationActionPolicy, WKWebpagePreferences) {
        // Said again for every navigation, whatever the configuration says.
        preferences.allowsContentJavaScript = false
        let target: WebNavigationPolicy.Target =
            switch action.targetFrame?.isMainFrame {
            case true?: .mainFrame
            case false?: .subframe
            case nil: .newWindow
            }
        let decision = resolve(
            url: action.request.url,
            clicked: action.navigationType == .linkActivated,
            target: target,
            download: action.shouldPerformDownload)
        return (decision, preferences)
    }

    public func webView(_: WKWebView, decidePolicyFor response: WKNavigationResponse) async
        -> WKNavigationResponsePolicy
    {
        guard response.isForMainFrame, let url = response.response.url, policy.isDocument(url) else {
            return .cancel
        }
        return .allow
    }

    /// A page asking for a window of its own gets none.
    public func webView(
        _: WKWebView, createWebViewWith _: WKWebViewConfiguration, for _: WKNavigationAction,
        windowFeatures _: WKWindowFeatures
    ) -> WKWebView? {
        nil
    }

    public func webView(_: WKWebView, didFinish _: WKNavigation?) {
        if phase == .loading {
            phase = .loaded
        }
    }

    public func webView(_: WKWebView, didFailProvisionalNavigation _: WKNavigation?, withError error: any Error) {
        loadFailed(error)
    }

    public func webView(_: WKWebView, didFail _: WKNavigation?, withError error: any Error) {
        loadFailed(error)
    }

    /// Only the pane's own load failing matters. A navigation this pane
    /// refused also ends up here, and that is no failure.
    private func loadFailed(_ error: any Error) {
        guard phase == .loading else { return }
        NSLog("Quill: the web view could not load its page: %@", String(describing: error))
        fail("This page could not be shown.")
    }

    public func webViewWebContentProcessDidTerminate(_: WKWebView) {
        fail("This page could not be shown.")
    }
}

/// The web view of the pane. Its menu offers only what stays in the pane or
/// goes through the pane's own rule for links: nothing that downloads a
/// file, opens a window or searches the web.
final class MessageWebView: WKWebView {
    static let allowedMenuItems: Set<String> = [
        "WKMenuItemIdentifierCopy",
        "WKMenuItemIdentifierCopyLink",
        "WKMenuItemIdentifierCopyImage",
        "WKMenuItemIdentifierOpenLink",
        "WKMenuItemIdentifierLookUp",
    ]

    override func willOpenMenu(_ menu: NSMenu, with event: NSEvent) {
        for item in menu.items where !Self.allowedMenuItems.contains(item.identifier?.rawValue ?? "") {
            menu.removeItem(item)
        }
        super.willOpenMenu(menu, with: event)
    }
}

/// WebKit's older switch for JavaScript, which also stops script the app
/// itself might inject. Deprecated in favour of the per-page preference,
/// which is set too; both are wanted here, so it is reached through a
/// protocol to keep the build free of the warning.
@MainActor
private protocol JavaScriptSwitch {
    func turnOffJavaScript(in preferences: WKPreferences)
}

@MainActor
private struct LegacyPreferences: JavaScriptSwitch {
    @available(macOS, deprecated: 11.0)
    func turnOffJavaScript(in preferences: WKPreferences) {
        preferences.javaScriptEnabled = false
    }

    static func turnOffJavaScript(in preferences: WKPreferences) {
        (LegacyPreferences() as any JavaScriptSwitch).turnOffJavaScript(in: preferences)
    }
}
