import CrossfadeModels
import XCTest

/// Only a conversion's own temporary files are swept at launch.
final class OutputFolderTests: XCTestCase {
    func testTemporaryNames() {
        XCTAssertTrue(OutputFolder.isStaleTemporary(name: ".song.flac.1234.tmp"))
        XCTAssertTrue(OutputFolder.isStaleTemporary(name: ".song.flac.1234-2.tmp"))
        XCTAssertFalse(OutputFolder.isStaleTemporary(name: "song.flac.1234.tmp"))
        XCTAssertFalse(OutputFolder.isStaleTemporary(name: ".song.flac"))
        XCTAssertFalse(OutputFolder.isStaleTemporary(name: ".song.flac.12a.tmp"))
        XCTAssertFalse(OutputFolder.isStaleTemporary(name: ".song.flac.1-2-3.tmp"))
        XCTAssertFalse(OutputFolder.isStaleTemporary(name: ".1234.tmp"))
        XCTAssertFalse(OutputFolder.isStaleTemporary(name: ".tmp"))
    }

    func testSweepRemovesOnlyTemporaryFiles() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        for name in [".song.flac.99.tmp", "song.flac", ".hidden"] {
            try Data("x".utf8).write(to: dir.appendingPathComponent(name))
        }
        let folder = OutputFolder(url: dir)
        XCTAssertEqual(folder.sweepStaleTemporaryFiles(), [".song.flac.99.tmp"])
        let left = try FileManager.default.contentsOfDirectory(atPath: dir.path(percentEncoded: false)).sorted()
        XCTAssertEqual(left, [".hidden", "song.flac"])
        XCTAssertEqual(folder.path(forName: "a b.wav"), dir.appendingPathComponent("a b.wav").path(percentEncoded: false))
    }
}
