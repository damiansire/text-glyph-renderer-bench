// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "POC2B",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "POC2B",
            path: "Sources/POC2B",
            resources: [.process("Rendering/Shaders")],
            swiftSettings: [
                .unsafeFlags(["-O", "-whole-module-optimization"], .when(configuration: .release)),
            ]
        )
    ]
)
