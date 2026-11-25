import Foundation

/// A failed call into the library: its status code and its message, which is written in the
/// app's own words and fit to show as it is.
public struct CrossfadeError: Error, LocalizedError, Equatable, Sendable {
    /// The `CF_ERR_*` status, or one of the negative codes below for failures on the Swift side.
    public let code: Int32
    public let message: String

    /// The library answered with JSON these models cannot read.
    public static let decodingCode: Int32 = -1
    /// The linked library speaks another version of the interface.
    public static let incompatibleCode: Int32 = -2
    /// The library said OK but left an out parameter empty.
    public static let missingResultCode: Int32 = -3

    public init(code: Int32, message: String) {
        self.code = code
        self.message = message
    }

    public var kind: Kind { Kind(code: code) }

    public var errorDescription: String? { message }

    /// The status codes of crossfade.h, by name.
    public enum Kind: Equatable, Sendable {
        case nullArgument, invalidUTF8, invalidJSON, refused, notFound, wrongThread, failed, panic
        case decoding, incompatible, missingResult
        case unknown

        init(code: Int32) {
            switch code {
            case 1: self = .nullArgument
            case 2: self = .invalidUTF8
            case 3: self = .invalidJSON
            case 4: self = .refused
            case 5: self = .notFound
            case 6: self = .wrongThread
            case 7: self = .failed
            case 8: self = .panic
            case CrossfadeError.decodingCode: self = .decoding
            case CrossfadeError.incompatibleCode: self = .incompatible
            case CrossfadeError.missingResultCode: self = .missingResult
            default: self = .unknown
            }
        }
    }
}
