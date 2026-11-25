import Foundation

/// How many jobs are in each state.
public struct QueueSummary: Codable, Equatable, Sendable {
    public var pending: Int
    public var running: Int
    public var finished: Int
    public var failed: Int
    public var cancelled: Int

    public init(
        pending: Int,
        running: Int,
        finished: Int,
        failed: Int,
        cancelled: Int
    ) {
        self.pending = pending
        self.running = running
        self.finished = finished
        self.failed = failed
        self.cancelled = cancelled
    }
}
