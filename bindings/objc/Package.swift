// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "FreshenObjC",
    platforms: [.macOS(.v11)],
    products: [
        .library(name: "FreshenObjC", targets: ["FreshenObjC"]),
    ],
    targets: [
        // The Rust static library (arm64 + x86_64) and its C header. Build it with
        // Tools/build-xcframework.sh before resolving this package; it is a build
        // artifact and is not checked in.
        .binaryTarget(name: "CFreshen", path: "CFreshen.xcframework"),

        .target(
            name: "FreshenObjC",
            dependencies: ["CFreshen"],
            path: "Sources/FreshenObjC",
            linkerSettings: [
                // What the Rust static library needs from the system. Confirm with
                //   cargo rustc -p freshen-ffi --release --crate-type staticlib \
                //     --target aarch64-apple-darwin -- --print native-static-libs
                .linkedFramework("Security"),
                .linkedFramework("CoreFoundation"),
                .linkedLibrary("iconv"),
            ]
        ),

        // Shared by the tests and the end-to-end parent application.
        .target(
            name: "FreshenObjCTestSupport",
            dependencies: ["FreshenObjC"],
            path: "Tests/FreshenObjCTestSupport"
        ),

        // The application that arms an update and exits, driven by the tests.
        .executableTarget(
            name: "FreshenObjCE2E",
            dependencies: ["FreshenObjC", "FreshenObjCTestSupport"],
            path: "Tests/FreshenObjCE2E"
        ),

        .testTarget(
            name: "FreshenObjCTests",
            dependencies: ["FreshenObjC", "FreshenObjCTestSupport"],
            path: "Tests/FreshenObjCTests"
        ),
    ]
)
