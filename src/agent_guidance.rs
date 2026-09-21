//! A discovery guide, not a second resource renderer. Keep it shared with mobile.
pub const GUIDE: &str = include_str!("issues/web/agent-guide.md");
pub const SCRIPT: &str = include_str!("issues/web/agent-guide.js");
const HEAD: &str = include_str!("issues/web/agent-guide-head.html");

pub fn decorate(html: &str) -> String {
    let guide = GUIDE
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    html.replacen("</head>", &format!("{HEAD}</head>"), 1)
        .replacen(
            "</body>",
            &format!("<pre hidden id=\"hey-boss-agent-guide\">{guide}</pre>\n</body>"),
            1,
        )
}
