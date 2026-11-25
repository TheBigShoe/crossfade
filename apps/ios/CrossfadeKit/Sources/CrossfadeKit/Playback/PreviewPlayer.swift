import AVFoundation
import CrossfadeFFI
import Foundation

/// The preview player: a `CfPlayer` made and driven on the main actor (the library ties it to
/// the thread that made it), its feed pulled by `AudioOutput`, a task waiting on its revision,
/// and the audio session's interruptions and route changes. Lives as long as the app.
@MainActor
public final class PreviewPlayer: PlaybackDriving {
    public private(set) var state: PlayerState?
    public var onStateChange: ((PlayerState) -> Void)?

    private var handle: PlayerHandle?
    private var output: AudioOutput?
    private var waiter: Task<Void, Never>?
    private var loadedPath: String?
    var resumeAfterInterruption = false
    var observers: [NSObjectProtocol] = []

    public init() {}

    /// Sets up the audio session (playback, so preview is heard with the silent switch on),
    /// makes the player at the device's rate and starts watching the session.
    public func start() throws {
        try AVAudioSession.sharedInstance().setCategory(.playback, mode: .default, options: [])
        try build()
        observeSession()
    }

    /// Stops everything and frees the player and its feed.
    public func shutdown() async {
        stopObservingSession()
        await tearDown()
    }

    // MARK: Transport

    public func load(path: String) throws {
        let player = try ready()
        loadedPath = path
        try FFI.check(path.withCString { cf_player_load(player, $0) })
        refresh()
    }

    public func play() throws { try command { cf_player_play($0) } }
    public func pause() throws { try command { cf_player_pause($0) } }
    public func toggle() throws { try command { cf_player_toggle($0) } }
    public func stop() throws { try command { cf_player_stop($0) } }
    public func seek(frame: UInt64) throws { try command { cf_player_seek($0, frame) } }
    public func scrub(frame: UInt64) throws { try command { cf_player_scrub($0, frame) } }
    public func endScrub() throws { try command { cf_player_end_scrub($0) } }

    public func position() -> UInt64 {
        guard let player = handle?.raw else { return 0 }
        var frames: UInt64 = 0
        if cf_player_position(player, &frames) == CF_OK {
            return frames
        }
        return state?.position ?? 0
    }

    private func ready() throws -> OpaquePointer {
        guard let player = handle?.raw else {
            throw CrossfadeError(
                code: CrossfadeError.missingResultCode, message: "Preview is not available.")
        }
        return player
    }

    private func command(_ call: (OpaquePointer) -> Int32) throws {
        let player = try ready()
        try FFI.check(call(player))
        refresh()
    }

    // MARK: State

    /// Reads the state and starts or pauses the device as the player asks.
    func refresh() {
        guard let player = handle?.raw else { return }
        guard let next = try? FFI.json(PlayerState.self, { cf_player_state(player, $0) }) else { return }
        let changed = next != state
        state = next
        syncDevice(next)
        if changed {
            onStateChange?(next)
        }
    }

    private func syncDevice(_ state: PlayerState) {
        guard let output = output else { return }
        if state.deviceRunning && !output.isRunning {
            do {
                try AVAudioSession.sharedInstance().setActive(true)
                try output.start()
            } catch {
                // Without a device nothing is pulled; pause so the transport says so.
                _ = cf_player_pause(handle?.raw)
            }
        } else if !state.deviceRunning && output.isRunning {
            output.pause()
            try? AVAudioSession.sharedInstance().setActive(false, options: [.notifyOthersOnDeactivation])
        }
    }

    // MARK: Building and tearing down

    private static func deviceSampleRate() -> Double {
        let rate = AVAudioSession.sharedInstance().sampleRate
        return rate > 0 ? rate : 48_000
    }

    private func build() throws {
        let rate = Self.deviceSampleRate()
        let channels = 2
        let player = try FFI.handle { cf_player_new(UInt32(rate.rounded()), UInt16(channels), $0) }
        let made: AudioOutput
        do {
            let feed = try FFI.handle { cf_player_take_feed(player, $0) }
            made = try AudioOutput(feed: feed, sampleRate: rate, channels: channels)
        } catch {
            _ = cf_player_destroy(player)
            throw error
        }
        handle = PlayerHandle(raw: player)
        output = made
        startWaiting(PlayerHandle(raw: player))
        refresh()
    }

    private func startWaiting(_ player: PlayerHandle) {
        waiter = Task.detached(priority: .utility) { [weak self] in
            var seen: UInt64 = 0
            while !Task.isCancelled {
                var now: UInt64 = seen
                if cf_player_wait(player.raw, seen, 250, &now) != CF_OK { break }
                if now != seen {
                    seen = now
                    await self?.refresh()
                }
            }
        }
    }

    /// Stops the device, waits for the waiting task to leave `cf_player_wait`, then frees the
    /// player (on this, its own thread) and, the device being stopped, the feed.
    private func tearDown() async {
        guard let player = handle else { return }
        let device = output
        handle = nil
        output = nil
        device?.pause()
        if let waiter = waiter {
            waiter.cancel()
            await waiter.value
        }
        waiter = nil
        _ = cf_player_destroy(player.raw)
        device?.release()
    }

    /// Makes a new player at the device's current rate, carrying over the file, the position
    /// and whether it was playing. For a route or format change.
    func rebuild() async {
        let path = loadedPath
        let at = position()
        let wasPlaying = state?.state == "playing"
        await tearDown()
        do {
            try build()
            if let path = path {
                try load(path: path)
                try seek(frame: at)
                if wasPlaying { try play() }
            }
        } catch {
            state = nil
        }
    }
}
