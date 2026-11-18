// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "hey-clipboard",
    platforms: [.macOS(.v15)],
    products: [.executable(name: "hey-clipboard", targets: ["HeyClipboard"])],
    dependencies: [.package(path: "../hey-talk")],
    targets: [
        .executableTarget(name: "HeyClipboard", dependencies: [.product(name: "TalkCore", package: "hey-talk")]),
        .testTarget(name: "HeyClipboardTests", dependencies: ["HeyClipboard"]),
    ],
    swiftLanguageModes: [.v5]
)
