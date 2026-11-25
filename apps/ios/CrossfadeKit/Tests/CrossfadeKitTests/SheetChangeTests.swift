import CrossfadeKit
import XCTest

/// Changes leave as the documented keys, with JSON null for "the default".
final class SheetChangeTests: XCTestCase {
    private func decoded(_ changes: [SheetChange]) throws -> [[String: Any]] {
        let text = try SheetChange.json(changes)
        let value = try JSONSerialization.jsonObject(with: Data(text.utf8), options: [.fragmentsAllowed])
        return try XCTUnwrap(value as? [[String: Any]])
    }

    func testKeysAndValues() throws {
        let list = try decoded([
            .container("flac"), .compressionLevel(8), .levelMode("gain"), .levelValue("-3"),
            .copy(true), .channels("5.1"), .adjustOpen(false),
        ])
        XCTAssertEqual(list.count, 7)
        XCTAssertEqual(list[0]["container"] as? String, "flac")
        XCTAssertEqual(list[1]["compression_level"] as? Int, 8)
        XCTAssertEqual(list[2]["level_mode"] as? String, "gain")
        XCTAssertEqual(list[3]["level_value"] as? String, "-3")
        XCTAssertEqual(list[4]["copy"] as? Bool, true)
        XCTAssertEqual(list[5]["channels"] as? String, "5.1")
        XCTAssertEqual(list[6]["adjust_open"] as? Bool, false)
    }

    func testDefaultsAreNull() throws {
        let list = try decoded([.bitrate(nil), .sampleRate(nil), .channels(nil), .compressionLevel(nil)])
        XCTAssertTrue(list[0]["bitrate"] is NSNull)
        XCTAssertTrue(list[1]["sample_rate"] is NSNull)
        XCTAssertTrue(list[2]["channels"] is NSNull)
        XCTAssertTrue(list[3]["compression_level"] is NSNull)
    }

    func testEachChangeIsItsOwnObject() throws {
        let text = try SheetChange.json([.output("/x/y.flac"), .overwrite(true)])
        XCTAssertTrue(text.hasPrefix("[{"))
        let list = try decoded([.output("/x/y.flac"), .overwrite(true)])
        XCTAssertEqual(list.map { $0.count }, [1, 1])
        XCTAssertEqual(list[0]["output"] as? String, "/x/y.flac")
    }
}
