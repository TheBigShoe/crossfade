import CrossfadeKit
import XCTest

/// Calls into the real library: the interface version, errors with their messages, and a
/// sheet refused for a file that does not exist. These need the XCFramework linked.
final class LibraryCallTests: XCTestCase {
    func testABIVersionMatches() throws {
        XCTAssertEqual(CrossfadeLibrary.abiVersion, CrossfadeLibrary.expectedABIVersion)
        XCTAssertNoThrow(try CrossfadeLibrary.checkABI())
        XCTAssertFalse(CrossfadeLibrary.version.isEmpty)
    }

    func testAbout() throws {
        let about = try LiveEngine().about()
        XCTAssertEqual(about.abiVersion, 1)
        XCTAssertTrue(about.lines[0].hasPrefix("Crossfade"))
    }

    func testAMissingFileIsUnreadableNotAnError() throws {
        let file = try LiveEngine().inspect(path: "/nonexistent/crossfade-test.wav")
        XCTAssertFalse(file.readable)
        XCTAssertNotNil(file.problem)
    }

    func testASheetForAMissingFileIsRefusedWithAMessage() {
        XCTAssertThrowsError(try ConvertSheet(path: "/nonexistent/crossfade-test.wav")) { error in
            let failure = error as? CrossfadeError
            XCTAssertEqual(failure?.kind, .refused)
            XCTAssertFalse(failure?.message.isEmpty ?? true)
        }
    }

    func testAnAnalysisOfAMissingFileEndsFailed() async throws {
        let analysis = try FileAnalysis(path: "/nonexistent/crossfade-test.wav")
        var last: AnalysisState?
        for await state in analysis.updates() {
            last = state
        }
        XCTAssertEqual(last?.state, "failed")
        XCTAssertNotNil(last?.failure)
    }

    func testAnEmptyQueue() throws {
        let queue = try ConversionQueue()
        let state = try queue.state()
        XCTAssertTrue(state.idle)
        XCTAssertTrue(state.jobs.isEmpty)
        XCTAssertThrowsError(try queue.cancel(99)) { error in
            XCTAssertEqual((error as? CrossfadeError)?.kind, .notFound)
        }
    }

    func testErrorKinds() {
        XCTAssertEqual(CrossfadeError(code: 3, message: "").kind, .invalidJSON)
        XCTAssertEqual(CrossfadeError(code: 6, message: "").kind, .wrongThread)
        XCTAssertEqual(CrossfadeError(code: 99, message: "").kind, .unknown)
        XCTAssertEqual(CrossfadeError(code: 4, message: "No.").errorDescription, "No.")
    }
}
