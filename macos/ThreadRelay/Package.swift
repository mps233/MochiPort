// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "ThreadRelayMac",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .executable(name: "ThreadRelayMac", targets: ["ThreadRelayMac"]),
    ],
    targets: [
        .executableTarget(
            name: "ThreadRelayMac",
            path: "Sources/ThreadRelayMac"
        ),
        .testTarget(
            name: "ThreadRelayMacTests",
            dependencies: ["ThreadRelayMac"],
            path: "Tests/ThreadRelayMacTests"
        ),
    ]
)
