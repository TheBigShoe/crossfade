import Foundation

/// An option with a value (`null` for the default) and the words to show for it.
public struct LabelledOption<Value: Codable & Equatable & Sendable>: Codable, Equatable, Sendable {
    public var label: String
    public var value: Value?

    public init(label: String, value: Value?) {
        self.label = label
        self.value = value
    }
}
