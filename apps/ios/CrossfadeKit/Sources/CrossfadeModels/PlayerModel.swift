import CrossfadeKit
import Foundation
import Observation

/// The transport: the player's state as the views show it, the commands, and the loaded
/// file's security scope, held for as long as the file is loaded.
@MainActor
@Observable
public final class PlayerModel {
    public private(set) var state: PlayerState?
    public private(set) var loadedURL: URL?
    /// The last command's failure, in the library's words.
    public private(set) var message: String?

    @ObservationIgnored private let player: (any PlaybackDriving)?
    @ObservationIgnored private var access: ScopedAccess?

    /// `player` is nil when preview could not start (no audio device): the transport hides.
    public init(player: (any PlaybackDriving)?) {
        self.player = player
        state = player?.state
        player?.onStateChange = { [weak self] next in
            self?.state = next
        }
    }

    public var isAvailable: Bool { player != nil }

    public var isPlaying: Bool { state?.state == "playing" }

    public var isLoaded: Bool {
        guard let phase = state?.state else { return false }
        return phase != "empty" && phase != "failed"
    }

    public var transportText: String { state?.transport ?? "" }

    public var duration: UInt64 { state?.duration ?? 0 }

    public var sampleRate: Int { state?.sampleRate ?? 0 }

    public var failure: String? { state?.state == "failed" ? state?.failure : nil }

    /// Loads `url` unless it is loaded already, leaving the previous file's scope.
    public func load(_ url: URL) {
        guard let player = player, url != loadedURL else { return }
        access?.end()
        access = ScopedAccess(url)
        loadedURL = url
        run { try player.load(path: url.path(percentEncoded: false)) }
    }

    /// Forgets the loaded file (it left the library) and leaves its scope.
    public func unload() {
        guard let player = player else { return }
        run { try player.stop() }
        access?.end()
        access = nil
        loadedURL = nil
    }

    public var holdsScope: Bool { access?.isActive == true }

    public func toggle() { run { try player?.toggle() } }
    public func stop() { run { try player?.stop() } }
    public func seek(to frame: UInt64) { run { try player?.seek(frame: frame) } }
    public func scrub(to frame: UInt64) { run { try player?.scrub(frame: frame) } }
    public func endScrub() { run { try player?.endScrub() } }

    /// Jumps by `seconds` (negative: back), held to the file.
    public func skip(seconds: Double) {
        guard sampleRate > 0 else { return }
        let target = Double(position()) + seconds * Double(sampleRate)
        let frame = UInt64(min(max(target, 0), Double(duration)))
        seek(to: frame)
    }

    /// The playhead in source frames; read once per display frame while playing.
    public func position() -> UInt64 {
        player?.position() ?? 0
    }

    private func run(_ command: () throws -> Void) {
        do {
            try command()
            message = nil
            if let next = player?.state { state = next }
        } catch {
            message = error.localizedDescription
        }
    }
}
