import AppKit
import WebKit

func resource(_ name: String) throws -> String {
    guard let url = Bundle.main.url(forResource: name, withExtension: nil) ?? Bundle.module.url(forResource: name, withExtension: nil, subdirectory: "Resources") else { throw StickyError("Missing renderer resource: \(name)") }
    return try String(contentsOf: url, encoding: .utf8)
}

final class NoteRenderer: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
    let web: WKWebView
    var ready: ((Result<CGFloat, Error>) -> Void)?
    var title = "Sticky"
    var errors: [String] = []
    private var timeout: DispatchWorkItem?
    private var loaded = false
    override init() {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        web = WKWebView(frame: .zero, configuration: config)
        super.init()
        web.setValue(false, forKey: "drawsBackground")
        web.navigationDelegate = self
        config.userContentController.add(self, name: "ready")
    }
    func load(_ markdown: String) throws {
        let document = try RenderedDocument(markdown)
        title = document.title; loaded = false; errors = []
        timeout?.cancel()
        web.configuration.userContentController.removeAllUserScripts()
        let controller = web.configuration.userContentController
        if document.html.contains("language-mermaid") {
            controller.addUserScript(WKUserScript(source: try resource("diagrams.js"), injectionTime: .atDocumentEnd, forMainFrameOnly: true))
        }
        controller.addUserScript(WKUserScript(source: try resource("ready.js"), injectionTime: .atDocumentEnd, forMainFrameOnly: true))
        let body = document.html.isEmpty ? "<p class='empty'>A little space for what matters.<br>Edit this note to add your focus.</p>" : document.html
        let html = """
        <!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
        <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src https: http: data:; font-src data:; base-uri 'none'; form-action 'none'">
        <style>\(try resource("note.css"))</style></head><body><article>\(body)</article></body></html>
        """
        web.loadHTMLString(html, baseURL: nil)
        let timeout = DispatchWorkItem { [weak self] in
            self?.finish(.failure(StickyError("Rendering took too long. Check the note's images and diagrams.")))
        }
        self.timeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + 20, execute: timeout)
    }
    private func finish(_ result: Result<CGFloat, Error>) {
        guard !loaded else { return }
        loaded = true; timeout?.cancel(); ready?(result)
    }
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard message.frameInfo.isMainFrame, let value = message.body as? [String: Any], let height = value["height"] as? Double else { return }
        errors = value["errors"] as? [String] ?? []
        finish(.success(CGFloat(height)))
    }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { finish(.failure(error)) }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { finish(.failure(error)) }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { finish(.failure(StickyError("The note renderer stopped. Reopen the note to reload it."))) }
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard action.navigationType == .linkActivated else {
            decisionHandler(action.request.url?.absoluteString == "about:blank" ? .allow : .cancel); return
        }
        guard let url = action.request.url else { decisionHandler(.cancel); return }
        if url.fragment != nil, url.absoluteString.hasPrefix("about:blank#") { decisionHandler(.allow); return }
        if ["https", "http", "mailto", "file", "t3", "slack", "obsidian"].contains(url.scheme?.lowercased() ?? "") { NSWorkspace.shared.open(url) }
        decisionHandler(.cancel)
    }
    func invalidate() {
        timeout?.cancel()
        web.stopLoading()
        web.configuration.userContentController.removeScriptMessageHandler(forName: "ready")
    }
}

// Export uses the same WebKit layout, with a stable glass-colored backing rather
// than copying private desktop pixels through the live window's material.
final class NoteExport {
    let renderer = NoteRenderer()
    let window: NSWindow
    let output: URL
    let dark: Bool
    let completion: (Result<[String: Any], Error>) -> Void
    init(markdown: String, output: URL, width: CGFloat, dark: Bool, completion: @escaping (Result<[String: Any], Error>) -> Void) throws {
        self.output = output; self.dark = dark; self.completion = completion
        window = NSWindow(contentRect: NSRect(x: -20000, y: -20000, width: width, height: 400), styleMask: .borderless, backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
        window.contentView = renderer.web
        window.orderBack(nil)
        renderer.ready = { [weak self] result in
            guard let self else { return }
            switch result {
            case .failure(let error): self.finish(.failure(error))
            case .success(let height):
                guard self.renderer.errors.isEmpty else { self.finish(.failure(StickyError(self.renderer.errors.joined(separator: "\n")))); return }
                guard height <= 8192 else { self.finish(.failure(StickyError("Note is too tall to export (maximum 8192 points)."))); return }
                self.snapshot(height: max(160, height))
            }
        }
        try renderer.load(markdown)
    }
    private func snapshot(height: CGFloat) {
        window.setContentSize(NSSize(width: window.frame.width, height: height))
        renderer.web.evaluateJavaScript("document.documentElement.style.overflow='hidden'; document.body.style.overflow='hidden'; document.documentElement.offsetHeight") { [weak self] _, error in
            guard let self else { return }
            if let error { self.finish(.failure(error)); return }
            let config = WKSnapshotConfiguration()
            config.rect = self.renderer.web.bounds
            config.snapshotWidth = NSNumber(value: Double(self.window.frame.width * 2))
            self.renderer.web.takeSnapshot(with: config) { [weak self] image, error in
                guard let self else { return }
                do {
                    guard let image else { throw error ?? StickyError("Could not capture the note.") }
                    let width = self.window.frame.width, total = height + 62
                    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(width * 2), pixelsHigh: Int(total * 2), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
                    bitmap.size = NSSize(width: width, height: total)
                    NSGraphicsContext.saveGraphicsState()
                    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
                    let bounds = NSRect(x: 0, y: 0, width: width, height: total)
                    NSBezierPath(roundedRect: bounds, xRadius: 22, yRadius: 22).addClip()
                    let top = self.dark ? NSColor(srgbRed: 0.19, green: 0.18, blue: 0.15, alpha: 1) : NSColor(srgbRed: 1, green: 0.97, blue: 0.88, alpha: 1)
                    let bottom = self.dark ? NSColor(srgbRed: 0.13, green: 0.13, blue: 0.12, alpha: 1) : NSColor(srgbRed: 0.97, green: 0.93, blue: 0.82, alpha: 1)
                    NSGradient(starting: bottom, ending: top)!.draw(in: bounds, angle: 90)
                    image.draw(in: NSRect(x: 0, y: 0, width: width, height: height))
                    let paragraph = NSMutableParagraphStyle(); paragraph.lineBreakMode = .byTruncatingTail
                    (self.renderer.title as NSString).draw(in: NSRect(x: 26, y: total - 44, width: width - 52, height: 26), withAttributes: [.font: NSFont.systemFont(ofSize: 19, weight: .semibold), .foregroundColor: self.dark ? NSColor(srgbRed: 0.93, green: 0.90, blue: 0.85, alpha: 1) : NSColor(srgbRed: 0.19, green: 0.17, blue: 0.14, alpha: 1), .paragraphStyle: paragraph])
                    NSColor.white.withAlphaComponent(self.dark ? 0.16 : 0.7).setStroke()
                    let rim = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 22, yRadius: 22)
                    rim.lineWidth = 1; rim.stroke()
                    NSGraphicsContext.restoreGraphicsState()
                    guard let png = bitmap.representation(using: .png, properties: [:]) else { throw StickyError("Could not encode PNG.") }
                    try png.write(to: self.output, options: .atomic)
                    self.finish(.success(["path":self.output.path, "title":self.renderer.title, "width":bitmap.pixelsWide, "height":bitmap.pixelsHigh]))
                } catch { self.finish(.failure(error)) }
            }
        }
    }
    private func finish(_ result: Result<[String: Any], Error>) {
        renderer.invalidate(); window.orderOut(nil); window.close(); completion(result)
    }
}
