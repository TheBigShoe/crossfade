import Foundation

/// A container the sheet offers, with the file extension it writes.
public struct ContainerOption: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var label: String
    /// The JSON's `extension`, a keyword in Swift.
    public var fileExtension: String

    public init(id: String, label: String, fileExtension: String) {
        self.id = id
        self.label = label
        self.fileExtension = fileExtension
    }

    // Matched after `.convertFromSnakeCase`, which leaves these keys as they are.
    enum CodingKeys: String, CodingKey {
        case id
        case label
        case fileExtension = "extension"
    }
}
