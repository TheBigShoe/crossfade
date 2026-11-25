import CrossfadeKit
import Foundation

/// A text field of the Adjust section: the text as the sheet holds it, its unit, why it
/// cannot be used, and how an edit is sent.
public struct TextRow: Equatable, Sendable {
    public var title: String
    public var text: String
    public var placeholder: String
    public var unit: String
    public var problem: String?
    public var send: Field

    /// Which sheet key an edit of this field goes to.
    public enum Field: Equatable, Sendable {
        case levelValue, trimStart, trimEnd, fadeIn, fadeOut
    }

    public init(title: String, text: String, placeholder: String, unit: String, problem: String?, send: Field) {
        self.title = title
        self.text = text
        self.placeholder = placeholder
        self.unit = unit
        self.problem = problem
        self.send = send
    }

    public func change(_ text: String) -> SheetChange {
        switch send {
        case .levelValue: return .levelValue(text)
        case .trimStart: return .trimStart(text)
        case .trimEnd: return .trimEnd(text)
        case .fadeIn: return .fadeIn(text)
        case .fadeOut: return .fadeOut(text)
        }
    }
}
