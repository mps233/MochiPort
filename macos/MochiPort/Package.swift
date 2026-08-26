// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "MochiPort",
    platforms: [
        .macOS("26.0"),
    ],
    products: [
        .executable(name: "MochiPort", targets: ["MochiPortMac"]),
    ],
    targets: [
        .executableTarget(
            name: "MochiPortMac",
            path: "Sources/MochiPortMac",
            resources: [
                .copy("Resources/ProviderLogos"),
                .copy("Resources/ClientLogos"),
            ]
        ),
        .testTarget(
            name: "MochiPortMacTests",
            dependencies: ["MochiPortMac"],
            path: "Tests/MochiPortMacTests"
        ),
    ]
)
