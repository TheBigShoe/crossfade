import CrossfadeKit
import Foundation
import Observation

/// The queue screen: the queue's state as rows, the commands, each job's input scope held
/// while the job may still read it, and the app kept awake while anything is to do.
@MainActor
@Observable
public final class QueueModel {
    public private(set) var state: QueueState?
    public private(set) var rows: [QueueRow] = []
    /// The last refusal, in the library's words.
    public private(set) var message: String?
    /// Counts jobs that finished or failed, so a view can confirm each with a haptic.
    public private(set) var finishedCount = 0
    public private(set) var failedCount = 0

    @ObservationIgnored private let queue: any QueueDriving
    @ObservationIgnored private let activity: (any ActivityAssertion)?
    @ObservationIgnored private let scopes = ScopeLedger<UInt64>()
    @ObservationIgnored private var inputs: [UInt64: URL] = [:]
    @ObservationIgnored private var watcher: Task<Void, Never>?

    public init(queue: any QueueDriving, activity: (any ActivityAssertion)?) {
        self.queue = queue
        self.activity = activity
    }

    /// Follows the queue's revisions for the life of the app.
    public func start() {
        guard watcher == nil else { return }
        let updates = queue.updates()
        watcher = Task { [weak self] in
            for await next in updates {
                self?.apply(next)
            }
        }
    }

    public var isBusy: Bool {
        guard let summary = state?.summary else { return false }
        return summary.pending + summary.running > 0
    }

    public var activeCount: Int {
        guard let summary = state?.summary else { return 0 }
        return summary.pending + summary.running
    }

    public var hasFinished: Bool { (state?.summary.finished ?? 0) > 0 }

    /// What quitting now would lose, in the library's words.
    public var quitWarning: String? { state?.quitWarning }

    /// Queues the sheet's job and holds `inputURL`'s scope until the job stops reading it.
    public func add(_ sheet: any SheetDriving, inputURL: URL) throws -> UInt64 {
        let id = try queue.add(sheet)
        inputs[id] = inputURL
        scopes.hold(id, [inputURL])
        refresh()
        return id
    }

    public func cancel(_ id: UInt64) { perform { try queue.cancel(id) } }

    public func retry(_ id: UInt64) {
        perform {
            let newId = try queue.retry(id)
            if let url = inputs.removeValue(forKey: id) {
                inputs[newId] = url
                scopes.hold(newId, [url])
            }
        }
    }

    public func remove(_ id: UInt64) { perform { try queue.remove(id) } }

    public func clearFinished() { perform { try queue.clearFinished() } }

    /// Whether this id's input scope is held (for the tests).
    public func holdsScope(for id: UInt64) -> Bool { scopes.keys.contains(id) }

    func apply(_ next: QueueState) {
        let before = state?.summary
        state = next
        rows = next.jobs.map { QueueRow(job: $0) }
        if let before = before {
            if next.summary.finished > before.finished { finishedCount += next.summary.finished - before.finished }
            if next.summary.failed > before.failed { failedCount += next.summary.failed - before.failed }
        }
        // A job that is no longer pending or running will not open its input again.
        let reading = Set(next.jobs.filter { $0.phase == .pending || $0.phase == .running }.map { $0.id })
        scopes.release(allBut: reading)
        for id in Array(inputs.keys) where !next.jobs.contains(where: { $0.id == id }) {
            inputs.removeValue(forKey: id)
        }
        activity?.setActive(isBusy)
    }

    private func perform(_ body: () throws -> Void) {
        do {
            try body()
            message = nil
        } catch {
            message = error.localizedDescription
        }
        refresh()
    }

    private func refresh() {
        if let next = try? queue.state() {
            apply(next)
        }
    }
}
