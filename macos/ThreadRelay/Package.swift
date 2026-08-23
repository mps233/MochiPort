// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "MochiPort",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(name: "MochiPort", targets: ["ThreadRelayMac"]),
    ],
    targets: [
        .executableTarget(
            name: "ThreadRelayMac",
            path: "Sources/ThreadRelayMac",
            resources: [
                .copy("Resources/ProviderLogos"),
                .copy("Resources/ClientLogos"),
            ]
        ),
        .testTarget(
            name: "ThreadRelayMacTests",
            dependencies: ["ThreadRelayMac"],
            path: "Tests/ThreadRelayMacTests"
        ),
    ]
)
