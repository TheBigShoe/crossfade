import CrossfadeKit
import Foundation

/// One option of a row, with the change that choosing it sends to the sheet.
public struct Choice: Equatable, Sendable {
    public var label: String
    public var change: SheetChange

    public init(label: String, change: SheetChange) {
        self.label = label
        self.change = change
    }
}
