import Foundation
import XCTest

/// The documents crossfade-ffi's snapshot tests pin, copied by apps/ios/scripts/pin_snapshots.py.
enum Fixture {
    static func data(_ name: String) throws -> Data {
        let url = Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Snapshots")
        let found = try XCTUnwrap(url, "missing Snapshots/\(name).json; run apps/ios/scripts/pin_snapshots.py")
        return try Data(contentsOf: found)
    }
}
