import CrossfadeFFI
import Foundation

/// The few helpers every call into C goes through: status to error, string out to `String`
/// (freed on every path), JSON out to a model.
enum FFI {
    typealias StringOut = UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>

    /// Takes ownership of a string the library returned: copies it and frees it.
    static func take(_ pointer: UnsafeMutablePointer<CChar>?) -> String? {
        guard let pointer = pointer else { return nil }
        defer { cf_string_free(pointer) }
        return String(cString: pointer)
    }

    /// The error for a failed status, with this thread's message. Call it right after the
    /// failed call: the message belongs to the thread and the next call clears it.
    static func error(for status: Int32) -> CrossfadeError {
        let pointer: UnsafeMutablePointer<CChar>? = cf_last_error_message()
        let message = take(pointer) ?? "Crossfade's library failed (status \(status))."
        return CrossfadeError(code: status, message: message)
    }

    static func check(_ status: Int32) throws {
        if status != CF_OK {
            throw error(for: status)
        }
    }

    static func missing(_ what: String) -> CrossfadeError {
        CrossfadeError(
            code: CrossfadeError.missingResultCode,
            message: "Crossfade's library returned no \(what).")
    }

    /// Calls `body` with a string out parameter and returns the string, freed.
    static func string(_ body: (StringOut) -> Int32) throws -> String {
        var out: UnsafeMutablePointer<CChar>? = nil
        let status = withUnsafeMutablePointer(to: &out) { body($0) }
        if status != CF_OK {
            let failure = error(for: status)
            // Written only on CF_OK, but never leak one if a future library does.
            _ = take(out)
            throw failure
        }
        guard let text = take(out) else { throw missing("document") }
        return text
    }

    /// Calls `body` with a string out parameter and decodes the JSON it returns.
    static func json<T: Decodable>(_ type: T.Type, _ body: (StringOut) -> Int32) throws -> T {
        let text = try string(body)
        return try CrossfadeJSON.decode(type, from: text)
    }

    /// Calls `body` with a handle out parameter and returns the handle.
    static func handle(_ body: (UnsafeMutablePointer<OpaquePointer?>) -> Int32) throws -> OpaquePointer {
        var out: OpaquePointer? = nil
        let status = withUnsafeMutablePointer(to: &out) { body($0) }
        try check(status)
        guard let handle = out else { throw missing("handle") }
        return handle
    }
}
