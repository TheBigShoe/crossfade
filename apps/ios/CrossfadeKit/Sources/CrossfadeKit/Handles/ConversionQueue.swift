import CrossfadeFFI
import Foundation

/// Owns the app's one `CfQueue`. Jobs run one at a time on the library's thread; any thread
/// may call in. Destroying it cancels every job and waits briefly for the running one to
/// remove its partial file.
public final class ConversionQueue: QueueDriving, @unchecked Sendable {
    let handle: OpaquePointer

    public init() throws {
        handle = try FFI.handle { cf_queue_new($0) }
    }

    deinit {
        cf_queue_destroy(handle)
    }

    /// Queues what the sheet asks for. A sheet with problems is refused with their messages.
    public func add(_ sheet: any SheetDriving) throws -> UInt64 {
        guard let live = sheet as? ConvertSheet else {
            throw CrossfadeError(
                code: CrossfadeError.missingResultCode,
                message: "This sheet does not belong to Crossfade's library.")
        }
        var id: UInt64 = 0
        try FFI.check(cf_queue_add(handle, live.handle, &id))
        return id
    }

    public func state() throws -> QueueState {
        try FFI.json(QueueState.self) { cf_queue_state(handle, $0) }
    }

    public func cancel(_ id: UInt64) throws {
        try FFI.check(cf_queue_cancel(handle, id))
    }

    /// Queues a failed or cancelled job again in its place; the old id is gone.
    public func retry(_ id: UInt64) throws -> UInt64 {
        var newId: UInt64 = 0
        try FFI.check(cf_queue_retry(handle, id, &newId))
        return newId
    }

    public func remove(_ id: UInt64) throws {
        try FFI.check(cf_queue_remove(handle, id))
    }

    public func clearFinished() throws {
        try FFI.check(cf_queue_clear_done(handle))
    }

    /// The state now and after every change, until the stream is dropped. The loop holds the
    /// queue alive, so it is never destroyed under a wait.
    public func updates() -> AsyncStream<QueueState> {
        RevisionStream.make(
            read: { try self.state() },
            revision: { $0.revision },
            wait: { since in
                var now: UInt64 = 0
                _ = cf_queue_wait(self.handle, since, RevisionStream.waitMilliseconds, &now)
            })
    }
}
