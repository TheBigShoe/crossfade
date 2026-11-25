import Foundation

/// Turns a blocking `cf_*_wait` into an `AsyncStream` of states. The loop runs on a detached
/// task, never the main actor, because the wait blocks for up to its timeout; the timeout is
/// short so a cancelled stream stops within it.
enum RevisionStream {
    static let waitMilliseconds: UInt32 = 500

    /// - read: the current state (a cheap JSON call).
    /// - revision: the state's revision.
    /// - wait: blocks until the revision differs from the one given or the timeout passes.
    /// - isFinal: after yielding a state for which this is true, the stream ends.
    static func make<State: Sendable>(
        read: @escaping @Sendable () throws -> State,
        revision: @escaping @Sendable (State) -> UInt64,
        wait: @escaping @Sendable (UInt64) -> Void,
        isFinal: @escaping @Sendable (State) -> Bool = { _ in false }
    ) -> AsyncStream<State> {
        AsyncStream<State> { continuation in
            let task = Task.detached(priority: .utility) {
                var seen: UInt64? = nil
                while !Task.isCancelled {
                    guard let state = try? read() else { break }
                    let now = revision(state)
                    if now != seen {
                        seen = now
                        continuation.yield(state)
                    }
                    if isFinal(state) { break }
                    wait(now)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
