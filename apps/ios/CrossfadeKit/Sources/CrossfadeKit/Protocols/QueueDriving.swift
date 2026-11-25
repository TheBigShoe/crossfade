import Foundation

/// The conversion queue. `updates` yields the state each time its revision moves.
public protocol QueueDriving: AnyObject {
    func add(_ sheet: any SheetDriving) throws -> UInt64
    func state() throws -> QueueState
    func cancel(_ id: UInt64) throws
    func retry(_ id: UInt64) throws -> UInt64
    func remove(_ id: UInt64) throws
    func clearFinished() throws
    func updates() -> AsyncStream<QueueState>
}
