import CrossfadeKit
import CrossfadeModels
import XCTest

/// The convert screen's model: output placed in the app's folder under the library's own name,
/// edits sent as the sheet's changes, queueing only when the sheet says it can.
final class ConvertModelTests: XCTestCase {
    private let folder = OutputFolder(url: URL(fileURLWithPath: "/Documents"))

    @MainActor
    private func model(_ state: SheetState) -> (ConvertModel, FakeSheet, FakeSheet) {
        let sheet = FakeSheet(state)
        let naming = FakeSheet(state)
        let model = ConvertModel(inputURL: URL(fileURLWithPath: "/elsewhere/in.wav"), folder: folder)
        model.attach(sheet: sheet, namingSheet: naming)
        return (model, sheet, naming)
    }

    @MainActor
    func testOutputGoesToTheFolderUnderTheLibrarysName() throws {
        let (model, sheet, _) = model(try Fixture.sheet())
        XCTAssertEqual(sheet.applied, [.output("/Documents/in-copy.wav")])
        XCTAssertEqual(model.form?.outputPath, "/Documents/in-copy.wav")
    }

    @MainActor
    func testAChoiceGoesToBothSheetsAndTheNameFollows() throws {
        let (model, sheet, naming) = model(try Fixture.sheet())
        model.send(.container("flac"))
        XCTAssertEqual(naming.applied, [.container("flac")])
        XCTAssertEqual(sheet.applied.last, .output("/Documents/in.flac"))
        XCTAssertEqual(model.form?.outputName, "in.flac")
    }

    @MainActor
    func testTheOverwriteSwitchOnlyGoesToTheShownSheet() throws {
        let (model, sheet, naming) = model(try Fixture.sheet())
        model.send(.overwrite(true))
        XCTAssertEqual(sheet.applied.last, .overwrite(true))
        XCTAssertTrue(naming.applied.isEmpty)
        XCTAssertEqual(model.form?.overwrite.isOn, true)
    }

    @MainActor
    func testATypedNameStopsFollowing() throws {
        let (model, sheet, _) = model(try Fixture.sheet())
        model.rename(to: "mine.aiff")
        XCTAssertFalse(model.followsDefaultName)
        XCTAssertEqual(sheet.applied.last, .output("/Documents/mine.aiff"))
        model.send(.container("flac"))
        XCTAssertNotEqual(sheet.applied.last, .output("/Documents/in.flac"))
    }

    @MainActor
    func testARefusedChangeShowsTheLibrarysMessage() throws {
        let (model, sheet, _) = model(try Fixture.sheet())
        sheet.refusal = CrossfadeError(code: 3, message: "That is not a container.")
        model.send(.container("mp3"))
        XCTAssertEqual(model.message, "That is not a container.")
    }

    @MainActor
    func testQueueingNeedsTheSheetsYes() throws {
        let queue = FakeQueue(try Fixture.queue())
        let queueModel = QueueModel(queue: queue, activity: nil)
        let (blocked, _, _) = model(try Fixture.sheet("sheet_problems"))
        XCTAssertNil(blocked.addToQueue(queueModel))
        XCTAssertTrue(queue.added.isEmpty)

        let (ready, _, _) = model(try Fixture.sheet())
        XCTAssertNotNil(ready.addToQueue(queueModel))
        XCTAssertEqual(queue.added.count, 1)
    }

    @MainActor
    func testARefusalFromTheQueueIsShown() throws {
        let queue = FakeQueue(try Fixture.queue())
        queue.refusal = CrossfadeError(code: 4, message: "Enter a gain in dB, such as -3.")
        let (model, _, _) = model(try Fixture.sheet())
        XCTAssertNil(model.addToQueue(QueueModel(queue: queue, activity: nil)))
        XCTAssertEqual(model.message, "Enter a gain in dB, such as -3.")
    }

    @MainActor
    func testASheetThatWillNotOpenSaysWhy() async throws {
        let engine = FakeEngine()
        engine.makeSheetResult = { throw CrossfadeError(code: 4, message: "unrecognised format") }
        let model = ConvertModel(inputURL: URL(fileURLWithPath: "/elsewhere/junk.wav"), folder: folder)
        await model.open(engine: engine)
        XCTAssertFalse(model.isOpening)
        XCTAssertNil(model.form)
        XCTAssertEqual(model.message, "unrecognised format")
    }
}
