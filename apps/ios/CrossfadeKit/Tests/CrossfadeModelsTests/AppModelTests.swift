import CrossfadeKit
import CrossfadeModels
import XCTest

/// Opening a file shows it, analyses it and loads it into the player; removing it lets go.
final class AppModelTests: XCTestCase {
    @MainActor
    func testOpenSelectAndRemove() async throws {
        let engine = FakeEngine()
        engine.files["/m/a.flac"] = FakeEngine.readable("/m/a.flac")
        let player = FakePlayer(state: try Fixture.decode(PlayerState.self, "player_shape"))
        let app = AppModel(
            engine: engine, queue: FakeQueue(try Fixture.queue()), player: player,
            store: MemoryStore(), bookmarks: PathBookmarks(), activity: nil,
            folder: OutputFolder(url: URL(fileURLWithPath: "/Documents")))

        await app.open([URL(fileURLWithPath: "/m/a.flac")])
        XCTAssertEqual(app.selectedItem?.title, "a.flac")
        XCTAssertEqual(player.commands, ["load /m/a.flac"])
        // The fake engine makes no analysis: the model says why instead.
        XCTAssertEqual(app.analysis?.failure, "no analysis in the fake")

        let id = try XCTUnwrap(app.selection)
        app.remove(ids: [id])
        XCTAssertNil(app.selection)
        XCTAssertNil(app.analysis)
        XCTAssertEqual(player.commands.last, "stop")
        XCTAssertTrue(app.library.items.isEmpty)
    }

    @MainActor
    func testAnUnreadableFileIsShownButNotAnalysedOrLoaded() async throws {
        let engine = FakeEngine()
        let player = FakePlayer(state: nil)
        let app = AppModel(
            engine: engine, queue: FakeQueue(try Fixture.queue()), player: player,
            store: MemoryStore(), bookmarks: PathBookmarks(), activity: nil,
            folder: OutputFolder(url: URL(fileURLWithPath: "/Documents")))
        await app.open([URL(fileURLWithPath: "/m/notes.txt")])
        XCTAssertEqual(app.selectedItem?.subtitle, "unrecognised format")
        XCTAssertNil(app.analysis)
        XCTAssertTrue(player.commands.isEmpty)
    }
}
