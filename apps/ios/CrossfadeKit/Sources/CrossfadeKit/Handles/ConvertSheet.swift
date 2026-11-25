import CrossfadeFFI
import Foundation

/// Owns a `CfSheet`. The library serialises calls on one sheet, so any thread may use it;
/// it is destroyed once, when the last reference goes.
public final class ConvertSheet: SheetDriving, @unchecked Sendable {
    let handle: OpaquePointer

    /// Opens a sheet for the file at `path`; reads its headers, so call it off the main actor.
    /// A file the inspector cannot read is refused with the inspector's problem.
    public init(path: String) throws {
        handle = try FFI.handle { out in path.withCString { cf_sheet_new($0, out) } }
    }

    deinit {
        cf_sheet_destroy(handle)
    }

    public func state() throws -> SheetState {
        try FFI.json(SheetState.self) { cf_sheet_state(handle, $0) }
    }

    public func apply(_ changes: [SheetChange]) throws {
        if changes.isEmpty { return }
        let text = try SheetChange.json(changes)
        try FFI.check(text.withCString { cf_sheet_apply(handle, $0) })
    }
}
