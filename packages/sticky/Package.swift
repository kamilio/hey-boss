// swift-tools-version: 6.0
import PackageDescription
import Foundation
let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path
let package = Package(
    name: "sticky",
    platforms: [.macOS("26.0")],
    products: [.executable(name: "sticky", targets: ["Sticky"])],
    targets: [
        .executableTarget(name: "Sticky", resources: [.copy("Resources")],
            linkerSettings: [.unsafeFlags(["-L\(root)/renderer/target/release", "-lsticky_renderer"])]),
        .testTarget(name: "StickyTests", dependencies: ["Sticky"])
    ],
    swiftLanguageModes: [.v5]
)
