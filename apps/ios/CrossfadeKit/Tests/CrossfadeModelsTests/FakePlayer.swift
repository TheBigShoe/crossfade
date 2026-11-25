import CrossfadeKit
import Foundation

/// A player that records commands and reports the state the test gives it.
@MainActor
final class FakePlayer: PlaybackDriving {
    var state: PlayerState?
    var onStateChange: ((PlayerState) -> Void)?
    private(set) var commands: [String] = []
    var at: UInt64 = 0

    init(state: PlayerState?) {
        self.state = state
    }

    func publish(_ next: PlayerState) {
        state = next
        onStateChange?(next)
    }

    func load(path: String) throws { commands.append("load \(path)") }
    func play() throws { commands.append("play") }
    func pause() throws { commands.append("pause") }
    func toggle() throws { commands.append("toggle") }
    func stop() throws { commands.append("stop") }
    func seek(frame: UInt64) throws { commands.append("seek \(frame)") }
    func scrub(frame: UInt64) throws { commands.append("scrub \(frame)") }
    func endScrub() throws { commands.append("end scrub") }
    func position() -> UInt64 { at }
}
