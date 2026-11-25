import Foundation

/// `cf_analysis_state`. `state` is `running done failed cancelled`; `lanes` is the waveform's lane count.
public struct AnalysisState: Codable, Equatable, Sendable {
    public var revision: UInt64
    public var state: String
    public var failure: String?
    public var framesDone: UInt64
    public var frames: UInt64?
    public var sampleRate: Int?
    public var channels: Int?
    public var lanes: Int
    public var loudness: Loudness
    public var readout: Readout

    public init(
        revision: UInt64,
        state: String,
        failure: String?,
        framesDone: UInt64,
        frames: UInt64?,
        sampleRate: Int?,
        channels: Int?,
        lanes: Int,
        loudness: Loudness,
        readout: Readout
    ) {
        self.revision = revision
        self.state = state
        self.failure = failure
        self.framesDone = framesDone
        self.frames = frames
        self.sampleRate = sampleRate
        self.channels = channels
        self.lanes = lanes
        self.loudness = loudness
        self.readout = readout
    }
}
