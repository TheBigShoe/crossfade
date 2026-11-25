import Foundation

/// One job of `cf_queue_state`. Progress while running (`fractionDone` may be null: then the
/// total is unknown), `frames` once finished, `message` (the engine's words) once failed.
public struct QueueJob: Codable, Equatable, Sendable, Identifiable {
    public var id: UInt64
    public var input: String
    public var output: String
    public var commandLine: String?
    /// `pending running finished failed cancelled`; kept as text so a new state decodes.
    public var state: String
    public var framesDone: UInt64?
    public var framesTotal: UInt64?
    public var fraction: Double?
    public var cancelling: Bool
    public var frames: UInt64?
    public var message: String?
    public var measuring: Bool
    public var normalization: Normalization?
    public var clipped: UInt64

    public init(
        id: UInt64, input: String, output: String, commandLine: String?, state: String,
        framesDone: UInt64?, framesTotal: UInt64?, fraction: Double?, cancelling: Bool,
        frames: UInt64?, message: String?, measuring: Bool, normalization: Normalization?,
        clipped: UInt64
    ) {
        self.id = id
        self.input = input
        self.output = output
        self.commandLine = commandLine
        self.state = state
        self.framesDone = framesDone
        self.framesTotal = framesTotal
        self.fraction = fraction
        self.cancelling = cancelling
        self.frames = frames
        self.message = message
        self.measuring = measuring
        self.normalization = normalization
        self.clipped = clipped
    }

    /// The state by name; nil for a state this app does not know yet.
    public var phase: JobPhase? { JobPhase(rawValue: state) }
}
