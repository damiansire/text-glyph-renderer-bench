// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "POC2A",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "POC2A",
            path: "Sources/POC2A",
            swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency"),
                .unsafeFlags(["-O", "-whole-module-optimization"], .when(configuration: .release)),
            ]
        )
    ]
)
