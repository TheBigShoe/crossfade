import Foundation

/// A convert sheet: its state, and edits applied all or none.
public protocol SheetDriving: AnyObject {
    func state() throws -> SheetState
    func apply(_ changes: [SheetChange]) throws
}
