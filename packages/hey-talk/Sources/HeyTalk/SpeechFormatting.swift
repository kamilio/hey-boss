import AppKit

// Chromium supplies AX font/color attributes rather than AppKit text-storage
// attributes. Preserve those captured styles; never re-render from plain Markdown.
enum SpeechFormatting {
    static func captured(_ target: SpeechTarget) -> NSAttributedString {
        let source = target.attributed ?? NSAttributedString(string: target.text)
        let result = NSMutableAttributedString(string: source.string)
        let paragraph = NSMutableParagraphStyle(); paragraph.lineSpacing = 4; paragraph.paragraphSpacing = 8
        result.addAttributes([.font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.labelColor,
                              .paragraphStyle: paragraph], range: NSRange(location: 0, length: result.length))
        source.enumerateAttributes(in: NSRange(location: 0, length: source.length)) { attrs, range, _ in
            var captured: [NSAttributedString.Key: Any] = [:]
            for key in [NSAttributedString.Key.font, .foregroundColor, .backgroundColor, .underlineStyle, .strikethroughStyle, .baselineOffset, .paragraphStyle, .kern, .link] {
                if let value = attrs[key] { captured[key] = value }
            }
            if let details = attrs[NSAttributedString.Key("AXFont")] as? [String: Any] {
                let size = (details["AXFontSize"] as? NSNumber)?.doubleValue ?? 14
                let name = details["AXFontName"] as? String ?? details["AXFontFamily"] as? String
                var font = name.flatMap { NSFont(name: $0, size: size) } ?? NSFont.systemFont(ofSize: size)
                var traits: NSFontTraitMask = []
                if details["AXFontBold"] as? Bool == true { traits.insert(.boldFontMask) }
                if details["AXFontItalic"] as? Bool == true { traits.insert(.italicFontMask) }
                if !traits.isEmpty { font = NSFontManager.shared.convert(font, toHaveTrait: traits) }
                captured[.font] = font
            }
            for (ax, native) in [("AXForegroundColor", NSAttributedString.Key.foregroundColor), ("AXBackgroundColor", .backgroundColor)] {
                if let value = attrs[NSAttributedString.Key(ax)], CFGetTypeID(value as CFTypeRef) == CGColor.typeID,
                   let color = NSColor(cgColor: value as! CGColor) { captured[native] = color }
            }
            if attrs[NSAttributedString.Key("AXUnderline")] as? Bool == true { captured[.underlineStyle] = NSUnderlineStyle.single.rawValue }
            if attrs[NSAttributedString.Key("AXStrikethrough")] as? Bool == true { captured[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
            result.addAttributes(captured, range: range)
        }
        return result
    }
}
