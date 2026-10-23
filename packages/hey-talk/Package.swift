// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "hey-talk",
    platforms: [.macOS(.v15)],
    products: [.executable(name: "hey-talk", targets: ["HeyTalk"])],
    targets: [
        .target(name: "TalkCore"),
        .target(name: "CSherpa", publicHeadersPath: "include"),
        .executableTarget(name: "HeyTalk", dependencies: ["TalkCore", "CSherpa"], resources: [.process("Prompts")]),
        .testTarget(name: "TalkCoreTests", dependencies: ["TalkCore"]),
        .testTarget(name: "HeyTalkTests", dependencies: ["HeyTalk"]),
    ],
    swiftLanguageModes: [.v5]
)
