import CrossfadeKit
import Foundation

/// One queue row as the view shows it, from the job's numbers. The library does not expose
/// the desktop's row wording, so the words are composed here from its figures.
public struct QueueRow: Identifiable, Equatable, Sendable {
    public var id: UInt64
    public var phase: JobPhase?
    public var title: String
    public var source: String
    public var status: String
    /// 0 to 1 while the total is known; nil while it is not (show an indeterminate bar).
    public var progress: Double?
    public var showsProgress: Bool
    /// The engine's whole message for a failed job.
    public var failure: String?
    public var canCancel: Bool
    public var canRetry: Bool
    public var canRemove: Bool
    /// The written file, once finished.
    public var outputURL: URL?
    public var commandLine: String?

    public init(job: QueueJob) {
        id = job.id
        phase = job.phase
        title = (job.output as NSString).lastPathComponent
        source = (job.input as NSString).lastPathComponent
        failure = job.message
        commandLine = job.commandLine
        let active = phase == .pending || phase == .running
        canCancel = active && !job.cancelling
        canRetry = phase == .failed || phase == .cancelled
        canRemove = phase != .running
        outputURL = phase == .finished ? URL(fileURLWithPath: job.output) : nil
        showsProgress = phase == .running
        progress = phase == .running ? job.fraction : nil
        status = Self.status(job)
    }

    static func status(_ job: QueueJob) -> String {
        switch job.phase {
        case .pending:
            return "Waiting"
        case .running:
            if job.cancelling { return "Cancelling…" }
            if job.measuring { return "Measuring loudness…" }
            if let fraction = job.fraction, job.framesTotal != nil {
                return "Converting · \(Int((fraction * 100).rounded()))%"
            }
            return "Converting · \((job.framesDone ?? 0).formatted()) frames"
        case .finished:
            var parts = ["Done"]
            if let frames = job.frames { parts.append("\(frames.formatted()) frames") }
            if let n = job.normalization {
                parts.append(String(format: "gain %+.1f dB", n.gainDb))
            }
            if job.clipped > 0 { parts.append("\(job.clipped.formatted()) samples clipped") }
            return parts.joined(separator: " · ")
        case .failed:
            return "Failed"
        case .cancelled:
            return "Cancelled"
        case nil:
            return job.state
        }
    }
}
