import Foundation

/// One inspector row: the probe key it shows, a stable id, the label and the values.
public struct InspectorRow: Codable, Equatable, Sendable, Identifiable {
    public var field: String
    public var id: String
    public var label: String
    public var value: String
    public var raw: String

    public init(
        field: String,
        id: String,
        label: String,
        value: String,
        raw: String
    ) {
        self.field = field
        self.id = id
        self.label = label
        self.value = value
        self.raw = raw
    }
}
