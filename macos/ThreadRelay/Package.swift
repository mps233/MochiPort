// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "ThreadRelayMac",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(name: "ThreadRelayMac", targets: ["ThreadRelayMac"]),
    ],
    targets: [
        .executableTarget(
            name: "ThreadRelayMac",
            path: "Sources/ThreadRelayMac",
            resources: [
                .copy("Resources/ProviderLogos"),
            ]
        ),
        .testTarget(
            name: "ThreadRelayMacTests",
            dependencies: ["ThreadRelayMac"],
            path: "Tests/ThreadRelayMacTests"
        ),
    ]
)
