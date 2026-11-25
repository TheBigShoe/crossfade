import CrossfadeKit
import Foundation

/// A queue whose state the test sets. Like the library's, an added or retried job is listed
/// as pending at once. It records the commands and can refuse.
final class FakeQueue: QueueDriving {
    var current: QueueState
    private(set) var added: [any SheetDriving] = []
    private(set) var cancelled: [UInt64] = []
    private(set) var removed: [UInt64] = []
    private(set) var cleared = 0
    var refusal: CrossfadeError?
    var nextId: UInt64 = 10
    private let template: QueueJob?

    init(_ state: QueueState, template: QueueJob? = nil) {
        current = state
        self.template = template
    }

    func add(_ sheet: any SheetDriving) throws -> UInt64 {
        if let refusal = refusal { throw refusal }
        added.append(sheet)
        nextId += 1
        if var job = template {
            job.id = nextId
            job.state = "pending"
            current.jobs.append(job)
        }
        return nextId
    }

    func state() throws -> QueueState { current }

    func cancel(_ id: UInt64) throws {
        if let refusal = refusal { throw refusal }
        cancelled.append(id)
    }

    func retry(_ id: UInt64) throws -> UInt64 {
        if let refusal = refusal { throw refusal }
        nextId += 1
        if let index = current.jobs.firstIndex(where: { $0.id == id }) {
            current.jobs[index].id = nextId
            current.jobs[index].state = "pending"
        }
        return nextId
    }

    func remove(_ id: UInt64) throws {
        if let refusal = refusal { throw refusal }
        removed.append(id)
    }

    func clearFinished() throws {
        cleared += 1
    }

    func updates() -> AsyncStream<QueueState> {
        AsyncStream { continuation in continuation.finish() }
    }
}
