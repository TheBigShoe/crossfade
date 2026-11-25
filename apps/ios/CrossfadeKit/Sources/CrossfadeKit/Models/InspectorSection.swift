import Foundation

/// One of the inspector's sections (File, Stream 0, Tags).
public struct InspectorSection: Codable, Equatable, Sendable, Identifiable {
    public var title: String
    public var rows: [InspectorRow]

    public init(
        title: String,
        rows: [InspectorRow]
    ) {
        self.title = title
        self.rows = rows
    }

    public var id: String { title }
}
