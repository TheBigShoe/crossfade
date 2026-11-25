import Foundation

/// Copy audio unchanged (a stream copy), offered only while `available`.
public struct CopyRow: Codable, Equatable, Sendable {
    public var value: Bool
    public var available: Bool

    public init(
        value: Bool,
        available: Bool
    ) {
        self.value = value
        self.available = available
    }
}
