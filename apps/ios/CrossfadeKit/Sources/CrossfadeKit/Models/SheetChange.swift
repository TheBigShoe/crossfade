import Foundation

/// One edit to the convert sheet, sent to `cf_sheet_apply` as the documented JSON key. The
/// sheet's own setter decides what the edit means; a value it does not take shows in its state.
public enum SheetChange: Equatable, Sendable {
    case preset(String)
    case container(String)
    case output(String)
    case copy(Bool)
    case format(String)
    case compressionLevel(Int?)
    case bitrate(Int?)
    case sampleRate(Int?)
    case channels(String?)
    case overwrite(Bool)
    case adjustOpen(Bool)
    case levelMode(String)
    case levelValue(String)
    case trimStart(String)
    case trimEnd(String)
    case fadeIn(String)
    case fadeOut(String)

    /// The JSON key and value, as `JSONSerialization` takes them (`NSNull` for null).
    var field: (key: String, value: Any) {
        func orNull<T>(_ value: T?) -> Any { value.map { $0 as Any } ?? NSNull() }
        switch self {
        case .preset(let id): return ("preset", id)
        case .container(let id): return ("container", id)
        case .output(let path): return ("output", path)
        case .copy(let on): return ("copy", on)
        case .format(let id): return ("format", id)
        case .compressionLevel(let level): return ("compression_level", orNull(level))
        case .bitrate(let bitsPerSecond): return ("bitrate", orNull(bitsPerSecond))
        case .sampleRate(let hertz): return ("sample_rate", orNull(hertz))
        case .channels(let layout): return ("channels", orNull(layout))
        case .overwrite(let on): return ("overwrite", on)
        case .adjustOpen(let open): return ("adjust_open", open)
        case .levelMode(let mode): return ("level_mode", mode)
        case .levelValue(let text): return ("level_value", text)
        case .trimStart(let text): return ("trim_start", text)
        case .trimEnd(let text): return ("trim_end", text)
        case .fadeIn(let text): return ("fade_in", text)
        case .fadeOut(let text): return ("fade_out", text)
        }
    }

    /// The changes as a JSON list of one-key objects, applied in turn.
    public static func json(_ changes: [SheetChange]) throws -> String {
        let list: [[String: Any]] = changes.map { change in
            let field = change.field
            return [field.key: field.value]
        }
        let data = try JSONSerialization.data(withJSONObject: list, options: [.sortedKeys])
        guard let text = String(data: data, encoding: .utf8) else {
            throw CrossfadeError(
                code: CrossfadeError.decodingCode, message: "Could not write the sheet's change.")
        }
        return text
    }
}
