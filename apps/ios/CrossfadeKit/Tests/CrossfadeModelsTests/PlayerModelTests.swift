import CrossfadeKit
import CrossfadeModels
import XCTest

/// The transport over a player: state from its callbacks, commands passed on, skips held to
/// the file, one load per file.
final class PlayerModelTests: XCTestCase {
    @MainActor
    func testStateAndCommands() throws {
        var ready = try Fixture.decode(PlayerState.self, "player_shape")
        let player = FakePlayer(state: ready)
        let model = PlayerModel(player: player)
        XCTAssertTrue(model.isAvailable)
        XCTAssertTrue(model.isLoaded)
        XCTAssertFalse(model.isPlaying)
        XCTAssertEqual(model.transportText, "0:00.0 / 0:02.0")

        let url = URL(fileURLWithPath: "/music/song.wav")
        model.load(url)
        model.load(url)
        model.toggle()
        ready.state = "playing"
        player.publish(ready)
        XCTAssertTrue(model.isPlaying)

        player.at = 44_100
        model.skip(seconds: -10)
        model.skip(seconds: 10)
        XCTAssertEqual(player.commands, ["load /music/song.wav", "toggle", "seek 0", "seek 88200"])
    }

    @MainActor
    func testNoPlayerHidesTheTransport() {
        let model = PlayerModel(player: nil)
        XCTAssertFalse(model.isAvailable)
        model.toggle()
        XCTAssertEqual(model.position(), 0)
    }
}
