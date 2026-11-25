import Foundation

/// `cf_player_state`. `state` is `empty loading ready playing paused ended failed`; frames are the source's.
public struct PlayerState: Codable, Equatable, Sendable {
    public var revision: UInt64
    public var state: String
    public var failure: String?
    public var position: UInt64
    public var duration: UInt64?
    public var sampleRate: Int?
    public var channels: Int?
    public var transport: String?
    public var scrubbing: Bool
    public var deviceRunning: Bool
    public var underruns: UInt64
    public var pulled: UInt64
    public var output: PlayerOutput

    public init(
        revision: UInt64,
        state: String,
        failure: String?,
        position: UInt64,
        duration: UInt64?,
        sampleRate: Int?,
        channels: Int?,
        transport: String?,
        scrubbing: Bool,
        deviceRunning: Bool,
        underruns: UInt64,
        pulled: UInt64,
        output: PlayerOutput
    ) {
        self.revision = revision
        self.state = state
        self.failure = failure
        self.position = position
        self.duration = duration
        self.sampleRate = sampleRate
        self.channels = channels
        self.transport = transport
        self.scrubbing = scrubbing
        self.deviceRunning = deviceRunning
        self.underruns = underruns
        self.pulled = pulled
        self.output = output
    }
}
