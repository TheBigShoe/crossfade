// swift-tools-version:5.9
// Swift 5 language mode on purpose: Swift 6's strict concurrency would turn the C handles'
// documented threading rules into compile errors the first Mac build cannot afford.
import PackageDescription

let package = Package(
    name: "CrossfadeKit",
    platforms: [.iOS(.v17)],
    products: [
        // The only code that touches C: models, handles, the player and its audio output.
        .library(name: "CrossfadeKit", targets: ["CrossfadeKit"]),
        // The app's view models: CrossfadeKit state in, what the views show out. No SwiftUI.
        .library(name: "CrossfadeModels", targets: ["CrossfadeModels"]),
    ],
    targets: [
        // Built by `just ios-framework` (scripts/build-xcframework.sh); git-ignored.
        .binaryTarget(name: "CrossfadeFFI", path: "Frameworks/CrossfadeFFI.xcframework"),
        .target(name: "CrossfadeKit", dependencies: ["CrossfadeFFI"]),
        .target(name: "CrossfadeModels", dependencies: ["CrossfadeKit"]),
        .testTarget(
            name: "CrossfadeKitTests",
            dependencies: ["CrossfadeKit"],
            resources: [.copy("Snapshots")]
        ),
        .testTarget(
            name: "CrossfadeModelsTests",
            dependencies: ["CrossfadeModels", "CrossfadeKit"],
            resources: [.copy("Snapshots")]
        ),
    ]
)
