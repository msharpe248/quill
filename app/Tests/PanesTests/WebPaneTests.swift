import AppKit
import Foundation
import Testing
import WebKit

@testable import Panes

/// Collects what a pane handed over to be opened.
@MainActor
private final class Opened {
    var urls: [URL] = []
}

@MainActor
@Suite(.serialized)
struct WebPaneTests {
    /// A page that reaches for the network every way a message can, each
    /// through a path of its own so the server can tell them apart.
    private func reachingPage(_ server: LocalServer) -> String {
        """
        <!doctype html>
        <html>
        <head>
        <title>before</title>
        <link rel="stylesheet" href="\(server.url("/style.css"))">
        <link rel="preconnect" href="\(server.url("/"))">
        <link rel="prefetch" href="\(server.url("/prefetch.html"))">
        <link rel="icon" href="\(server.url("/icon.png"))">
        <style>
        @import url("\(server.url("/import.css"))");
        @font-face { font-family: remote; src: url("\(server.url("/font.woff"))"); }
        body { background: url("\(server.url("/css-url.png"))"); font-family: remote; }
        </style>
        <script src="\(server.url("/script.js"))"></script>
        <script>
        document.title = "script ran";
        fetch("\(server.url("/fetched-by-script"))");
        new Image().src = "\(server.url("/image-by-script.png"))";
        </script>
        </head>
        <body background="\(server.url("/body-background.png"))" onload="fetch('\(server.url("/fetched-on-load"))')">
        <p>Hello from example.com</p>
        <img src="\(server.url("/pixel.png"))" onerror="fetch('\(server.url("/fetched-on-error"))')">
        <img srcset="\(server.url("/srcset.png")) 2x">
        <iframe src="\(server.url("/frame.html"))"></iframe>
        <video poster="\(server.url("/poster.png"))"></video>
        <audio src="\(server.url("/sound.mp3"))" autoplay></audio>
        <object data="\(server.url("/object.bin"))"></object>
        <embed src="\(server.url("/embed.bin"))">
        <svg><image href="\(server.url("/svg-image.png"))" width="1" height="1"/></svg>
        <form action="\(server.url("/form"))" method="post"><input type="image" src="\(server.url("/input.png"))"></form>
        <a href="\(server.url("/link"))" ping="\(server.url("/ping"))">a link</a>
        </body>
        </html>
        """
    }

    private func pane(_ file: URL, remote: Bool, opened: Opened = Opened()) -> WebPane {
        WebPane(file: file, title: "A page", allowsRemoteContent: remote) { opened.urls.append($0) }
    }

    // The milestone's acceptance test.
    @Test func nothingIsAskedOfTheNetworkWhileRemoteContentIsBlocked() async throws {
        let scratch = try Scratch()
        let server = try LocalServer()
        defer { server.stop() }
        let file = try scratch.write(reachingPage(server), to: "message.html")

        let pane = pane(file, remote: false)
        defer { pane.tearDown() }
        #expect(pane.phase == .preparing)
        try await wait(for: "the page to load") { pane.phase == .loaded || pane.phase == .failed }
        #expect(pane.phase == .loaded)
        // Loaded, and then time enough for anything slow to reach out.
        let quiet = try await holds(for: 2) { server.connections == 0 }
        #expect(quiet)
        #expect(server.connections == 0)
        #expect(server.paths == [])
        #expect(pane.webView.title == "before")
        #expect(pane.summary.hasPrefix("remote content blocked, loaded"))
    }

    // The same page with remote content allowed: the pictures and styles
    // are asked for, which also shows the test above could have seen a
    // request. Script still does not run and frames still do not load.
    @Test func remoteContentLoadsWhenAllowedButScriptNeverRuns() async throws {
        let scratch = try Scratch()
        let server = try LocalServer()
        defer { server.stop() }
        let file = try scratch.write(reachingPage(server), to: "message.html")

        let pane = pane(file, remote: true)
        defer { pane.tearDown() }
        try await wait(for: "the page to load") { pane.phase == .loaded || pane.phase == .failed }
        #expect(pane.phase == .loaded)
        try await wait(for: "the picture and the stylesheet to be asked for") {
            let paths = server.paths
            return paths.contains("/pixel.png") && paths.contains("/style.css")
        }
        #expect(server.connections > 0)
        // Time for anything that should not happen to happen.
        _ = try await holds(for: 1) { true }
        let paths = Set(server.paths)
        let forbidden: Set<String> = [
            "/script.js", "/fetched-by-script", "/image-by-script.png", "/fetched-on-load", "/fetched-on-error",
            "/frame.html", "/prefetch.html", "/ping", "/form", "/link", "/object.bin", "/embed.bin",
        ]
        #expect(paths.isDisjoint(with: forbidden), "asked for \(paths.intersection(forbidden).sorted())")
        #expect(pane.webView.title == "before")
        #expect(pane.summary.hasPrefix("remote content allowed, loaded"))
    }

    @Test func aPageThatIsNotThereLoadsNothingAndSaysSo() async throws {
        let scratch = try Scratch()
        let pane = pane(scratch.file("missing.html"), remote: false)
        defer { pane.tearDown() }
        #expect(pane.phase == .failed)
        #expect(pane.message != nil)
        let stays = try await holds(for: 0.3) { pane.phase == .failed && pane.webView.url == nil }
        #expect(stays)
        #expect(pane.summary.hasPrefix("remote content blocked, failed, says "))
    }

    @Test func aPaneClosedBeforeItsRulesAreReadyLoadsNothing() async throws {
        let scratch = try Scratch()
        let file = try scratch.write("<p>Hello</p>", to: "message.html")
        let pane = pane(file, remote: false)
        #expect(pane.phase == .preparing)
        pane.tearDown()
        let stays = try await holds(for: 0.5) { pane.phase == .preparing && pane.webView.url == nil }
        #expect(stays)
    }

    @Test func theWebViewKeepsEveryKeyButEsc() throws {
        let scratch = try Scratch()
        let file = try scratch.write("<p>Hello</p>", to: "message.html")
        let pane = pane(file, remote: false)
        defer { pane.tearDown() }
        #expect(pane.kind == .web)
        // Arrows and space scroll the page; the pane takes none of them.
        for key in PaneKey.allCases {
            #expect(!pane.handleKey(key))
        }
    }

    @Test func theConfigurationRunsNoScriptAndKeepsNothing() {
        let configuration = WebPane.configuration()
        #expect(configuration.defaultWebpagePreferences.allowsContentJavaScript == false)
        #expect(configuration.websiteDataStore.isPersistent == false)
        #expect(configuration.preferences.javaScriptCanOpenWindowsAutomatically == false)
        #expect(configuration.preferences.value(forKey: "javaScriptEnabled") as? Bool == false)
    }

    // MARK: - Where the view may go

    @Test func aClickedLinkIsHandedOverAndNeverFollowed() throws {
        let scratch = try Scratch()
        let file = try scratch.write("<p>Hello</p>", to: "message.html")
        let opened = Opened()
        let pane = pane(file, remote: false, opened: opened)
        defer { pane.tearDown() }

        let web = try #require(URL(string: "https://example.com/"))
        #expect(pane.resolve(url: web, clicked: true, target: .mainFrame) == .cancel)
        #expect(opened.urls == [web])
        // The same link asking for a window of its own, as most links in
        // mail do.
        #expect(pane.resolve(url: web, clicked: true, target: .newWindow) == .cancel)
        let mail = try #require(URL(string: "mailto:ada@example.com"))
        #expect(pane.resolve(url: mail, clicked: true, target: .mainFrame) == .cancel)
        #expect(opened.urls == [web, web, mail])
        #expect(pane.refused == [])
    }

    @Test func everythingElseIsRefused() throws {
        let scratch = try Scratch()
        let file = try scratch.write("<p>Hello</p>", to: "message.html")
        let opened = Opened()
        let pane = pane(file, remote: true, opened: opened)
        defer { pane.tearDown() }

        let refused = [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ftp://example.com/file",
            "tel:+15550100",
            "x-apple.systempreferences:com.apple.preference.security",
            scratch.file("other.html").absoluteString,
            // The document itself with a question to it is not the document.
            file.absoluteString + "?x=1",
        ]
        for address in refused {
            let url = try #require(URL(string: address))
            #expect(pane.resolve(url: url, clicked: true, target: .mainFrame) == .cancel, "\(address)")
            #expect(pane.resolve(url: url, clicked: true, target: .newWindow) == .cancel, "\(address)")
        }
        // Nobody clicked: a redirect, a refresh, a form, a frame.
        let web = try #require(URL(string: "https://example.com/"))
        #expect(pane.resolve(url: web, clicked: false, target: .mainFrame) == .cancel)
        #expect(pane.resolve(url: web, clicked: false, target: .subframe) == .cancel)
        #expect(pane.resolve(url: web, clicked: false, target: .newWindow) == .cancel)
        // A download, clicked or not.
        #expect(pane.resolve(url: web, clicked: true, target: .mainFrame, download: true) == .cancel)
        #expect(pane.resolve(url: nil, clicked: true, target: .mainFrame) == .cancel)
        // The document may not be put in a frame of itself either.
        #expect(pane.resolve(url: file, clicked: false, target: .subframe) == .cancel)

        #expect(opened.urls == [])
        #expect(pane.refused.count == refused.count * 2 + 6)
        #expect(pane.refused.first == "file:///etc/passwd")
    }

    @Test func theDocumentItselfMayLoadAndBeMovedAboutIn() throws {
        let document = URL(fileURLWithPath: "/tmp/quill-view/message.html")
        let policy = WebNavigationPolicy(document: document)
        #expect(policy.decide(url: document, clicked: false, target: .mainFrame) == .allow)
        let section = try #require(URL(string: document.absoluteString + "#section"))
        #expect(policy.decide(url: section, clicked: true, target: .mainFrame) == .allow)
        // The same file by a longer road.
        let longer = try #require(URL(string: "file:///tmp/quill-view/../quill-view/message.html"))
        #expect(policy.decide(url: longer, clicked: false, target: .mainFrame) == .allow)
        let sibling = URL(fileURLWithPath: "/tmp/quill-view/other.html")
        #expect(policy.decide(url: sibling, clicked: true, target: .mainFrame) == .refuse)
        #expect(policy.decide(url: document, clicked: false, target: .newWindow) == .refuse)
        #expect(policy.decide(url: document, clicked: false, target: .mainFrame, download: true) == .refuse)
    }

    @Test func theRefusedListDoesNotGrowWithoutEnd() throws {
        let scratch = try Scratch()
        let file = try scratch.write("<p>Hello</p>", to: "message.html")
        let pane = pane(file, remote: false)
        defer { pane.tearDown() }
        for number in 0..<200 {
            _ = pane.resolve(url: URL(string: "file:///nowhere/\(number)"), clicked: false, target: .mainFrame)
        }
        #expect(pane.refused.count == 50)
        #expect(pane.refused.last == "file:///nowhere/199")
    }

    // The real web view, sent somewhere by the page and then by hand.
    @Test func aPageCannotSendTheViewAnywhere() async throws {
        let scratch = try Scratch()
        let server = try LocalServer()
        defer { server.stop() }
        let secret = try scratch.write("<title>secret</title>", to: "secret.html")
        let file = try scratch.write(
            """
            <!doctype html><title>before</title>
            <meta http-equiv="refresh" content="0; url=\(server.url("/refreshed"))">
            <p>Hello</p>
            """, to: "message.html")
        let opened = Opened()
        let pane = pane(file, remote: true, opened: opened)
        defer { pane.tearDown() }
        try await wait(for: "the page to load") { pane.phase == .loaded || pane.phase == .failed }
        #expect(pane.phase == .loaded)
        try await wait(for: "the refresh to be refused") { pane.refused.contains(server.url("/refreshed")) }

        // Another file, even one beside the document.
        pane.webView.load(URLRequest(url: secret))
        try await wait(for: "the other file to be refused") { pane.refused.contains(secret.absoluteString) }
        pane.webView.load(URLRequest(url: URL(fileURLWithPath: "/etc/passwd")))
        try await wait(for: "a file elsewhere to be refused") { pane.refused.contains("file:///etc/passwd") }

        _ = try await holds(for: 0.5) { true }
        #expect(pane.webView.title == "before")
        #expect(pane.webView.url?.lastPathComponent == "message.html")
        #expect(pane.phase == .loaded)
        #expect(opened.urls == [])
        #expect(server.connections == 0)
    }

    // MARK: - The reading pane

    private func readingPane(_ file: URL, title: String = "A subject", header: [String]) -> WebPane {
        WebPane(file: file, title: title, allowsRemoteContent: false, readingHeader: header) { _ in }
    }

    @Test func theReadingPaneHasItsLinesAboveThePageAndNoCloseButton() async throws {
        let scratch = try Scratch()
        let file = try scratch.write("<!doctype html><title>before</title><p>Hello</p>", to: "message.html")
        let lines = ["From: Ana Ruiz <ana.ruiz@example.com>", "To: Robin Vale <robin@example.com>"]
        let pane = readingPane(file, header: lines)
        defer { pane.tearDown() }
        #expect(pane.isReading && !pane.isClosable)
        #expect(pane.headerLines == lines)
        #expect(pane.kind == .web)
        // The lines are labels in the pane, outside the web view.
        let labels = allSubviews(of: pane).compactMap { $0 as? NSTextField }.map(\.stringValue)
        for line in lines + ["A subject"] {
            #expect(labels.contains(line), "\(line) is not shown")
        }
        #expect(allSubviews(of: pane.webView).compactMap { $0 as? NSTextField }.isEmpty)
        let close = allSubviews(of: pane).compactMap { $0 as? NSButton }.first
        #expect(close?.isHidden == true)

        // It is told once when there is something to see.
        var settled = 0
        pane.onSettled = { settled += 1 }
        try await wait(for: "the page to load") { pane.phase == .loaded || pane.phase == .failed }
        #expect(pane.phase == .loaded)
        #expect(settled == 1)
        #expect(pane.document == file)
        // Held to the same rules as any page: no script, nothing remote.
        #expect(pane.webView.title == "before")
        #expect(!pane.allowsRemoteContent)
        #expect(pane.summary == "remote content blocked, loaded")

        // The pane asked for by itself is as it was: closable, no lines.
        let plain = self.pane(file, remote: false)
        defer { plain.tearDown() }
        #expect(!plain.isReading && plain.isClosable && plain.headerLines.isEmpty)
    }

    @Test func aReadingPaneWithNothingToSayAboveItsPageSaysNothing() throws {
        let scratch = try Scratch()
        let file = try scratch.write("<p>No message selected</p>", to: "message.html")
        let pane = readingPane(file, title: "", header: [])
        defer { pane.tearDown() }
        #expect(pane.isReading && !pane.isClosable)
        let labels = allSubviews(of: pane).compactMap { $0 as? NSTextField }
        // Whatever labels there are, none is on show.
        #expect(labels.allSatisfy { $0.stringValue.isEmpty || $0.isHiddenOrHasHiddenAncestor })
    }

    // The page is scrolled from outside the web view, with no script: the
    // top of this one is red and everything a screenful down is blue.
    @Test func theReadingPaneScrollsByAPageAndBack() async throws {
        let scratch = try Scratch()
        let file = try scratch.write(
            """
            <!doctype html><title>before</title>
            <body style="margin:0">
            <div style="height:150px;background:rgb(255,0,0)"></div>
            <div style="height:4000px;background:rgb(0,0,255)"></div>
            """, to: "message.html")
        let pane = readingPane(file, header: ["From: Ana Ruiz <ana.ruiz@example.com>"])
        defer { pane.tearDown() }
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 300), styleMask: [.borderless],
            backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.contentView = pane
        pane.layoutSubtreeIfNeeded()
        // Asked before there is a page, nothing happens.
        pane.scroll(.pageDown)
        #expect(pane.scrolls == 0)
        try await wait(for: "the page to load") { pane.phase == .loaded || pane.phase == .failed }
        #expect(pane.phase == .loaded)
        #expect(!window.isVisible)
        // The keyboard is not the web view's, and scrolling does not take it.
        #expect((window.firstResponder as? NSView)?.isDescendant(of: pane.webView) != true)

        try await wait(for: "the top of the page to be drawn") { await topColour(of: pane) == .red }
        pane.scroll(.pageDown)
        try await wait(for: "the page to scroll down") { await topColour(of: pane) == .blue }
        pane.scroll(.pageUp)
        try await wait(for: "the page to scroll back") { await topColour(of: pane) == .red }
        #expect(pane.scrolls == 2)
        #expect(pane.summary == "remote content blocked, loaded, scrolled 2")
        #expect((window.firstResponder as? NSView)?.isDescendant(of: pane.webView) != true)
        #expect(pane.webView.title == "before")
    }

    private enum Colour { case red, blue, other }

    /// The colour at the top of what the page shows, from the web view's
    /// own picture of itself.
    private func topColour(of pane: WebPane) async -> Colour {
        guard let picture = await pane.picture(),
            let image = picture.cgImage(forProposedRect: nil, context: nil, hints: nil)
        else { return .other }
        let pixel = NSBitmapImageRep(cgImage: image).colorAt(x: image.width / 2, y: 4)?
            .usingColorSpace(.sRGB)
        guard let pixel else { return .other }
        if pixel.redComponent > 0.8, pixel.blueComponent < 0.3 { return .red }
        if pixel.blueComponent > 0.8, pixel.redComponent < 0.3 { return .blue }
        return .other
    }

    private func allSubviews(of view: NSView) -> [NSView] {
        view.subviews + view.subviews.flatMap { allSubviews(of: $0) }
    }

    // A real click on a real link, in a window that is never put on screen.
    @Test func clickingALinkOpensItElsewhere() async throws {
        let scratch = try Scratch()
        let server = try LocalServer()
        defer { server.stop() }
        let file = try scratch.write(
            """
            <!doctype html><title>before</title>
            <body style="margin:0">
            <a href="\(server.url("/clicked"))" target="_blank"
               style="display:block;width:2000px;height:2000px">a link</a>
            """, to: "message.html")
        let opened = Opened()
        let pane = pane(file, remote: true, opened: opened)
        defer { pane.tearDown() }
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 300), styleMask: [.borderless],
            backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.contentView = pane
        pane.layoutSubtreeIfNeeded()
        try await wait(for: "the page to load") { pane.phase == .loaded || pane.phase == .failed }
        #expect(!window.isVisible)
        #expect(window.makeFirstResponder(pane.focusTarget))
        #expect((window.firstResponder as? NSView)?.isDescendant(of: pane) == true)

        let middle = pane.webView.convert(NSPoint(x: pane.webView.bounds.midX, y: pane.webView.bounds.midY), to: nil)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            let event = try #require(
                NSEvent.mouseEvent(
                    with: type, location: middle, modifierFlags: [],
                    timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                    context: nil, eventNumber: 0, clickCount: 1, pressure: type == .leftMouseDown ? 1 : 0))
            if type == .leftMouseDown {
                pane.webView.mouseDown(with: event)
            } else {
                pane.webView.mouseUp(with: event)
            }
        }
        try await wait(for: "the click to reach the link") { !opened.urls.isEmpty }
        #expect(opened.urls.map(\.absoluteString) == [server.url("/clicked")])
        _ = try await holds(for: 0.5) { true }
        #expect(server.paths == [])
        #expect(pane.webView.title == "before")
    }
}
