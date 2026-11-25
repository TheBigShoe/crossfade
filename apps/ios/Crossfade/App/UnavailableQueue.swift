import CrossfadeKit
import Foundation

/// Stands in for the queue if the library could not make one, so the app still opens and
/// says why converting is off.
final class UnavailableQueue: QueueDriving {
    private let reason: String

    init(reason: String) {
        self.reason = reason
    }

    private var refusal: CrossfadeError { CrossfadeError(code: 7, message: reason) }

    private var empty: QueueState {
        QueueState(
            revision: 0, idle: true,
            summary: QueueSummary(pending: 0, running: 0, finished: 0, failed: 0, cancelled: 0),
            quitWarning: nil, jobs: [])
    }

    func add(_ sheet: any SheetDriving) throws -> UInt64 { throw refusal }
    func state() throws -> QueueState { empty }
    func cancel(_ id: UInt64) throws { throw refusal }
    func retry(_ id: UInt64) throws -> UInt64 { throw refusal }
    func remove(_ id: UInt64) throws { throw refusal }
    func clearFinished() throws {}

    func updates() -> AsyncStream<QueueState> {
        let state = empty
        return AsyncStream { continuation in
            continuation.yield(state)
            continuation.finish()
        }
    }
}
