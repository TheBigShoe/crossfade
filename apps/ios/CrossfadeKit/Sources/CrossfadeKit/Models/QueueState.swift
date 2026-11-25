import Foundation

/// `cf_queue_state`.
public struct QueueState: Codable, Equatable, Sendable {
    public var revision: UInt64
    public var idle: Bool
    public var summary: QueueSummary
    public var quitWarning: String?
    public var jobs: [QueueJob]

    public init(
        revision: UInt64,
        idle: Bool,
        summary: QueueSummary,
        quitWarning: String?,
        jobs: [QueueJob]
    ) {
        self.revision = revision
        self.idle = idle
        self.summary = summary
        self.quitWarning = quitWarning
        self.jobs = jobs
    }
}
