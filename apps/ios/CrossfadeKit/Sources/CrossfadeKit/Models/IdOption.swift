import Foundation

/// An option named by an id (presets, formats).
public struct IdOption: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var label: String

    public init(
        id: String,
        label: String
    ) {
        self.id = id
        self.label = label
    }
}
