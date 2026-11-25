import Foundation

/// A piece of the readout line; `warn` asks for the warning colour.
public struct ReadoutSegment: Codable, Equatable, Sendable {
    public var text: String
    public var warn: Bool

    public init(
        text: String,
        warn: Bool
    ) {
        self.text = text
        self.warn = warn
    }
}
