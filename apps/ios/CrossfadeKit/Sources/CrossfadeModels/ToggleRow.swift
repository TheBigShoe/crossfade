import CrossfadeKit
import Foundation

/// A switch row of the convert form; `change(on)` is what flipping it sends.
public struct ToggleRow: Equatable, Sendable {
    public var title: String
    public var visible: Bool
    public var enabled: Bool
    public var isOn: Bool
    public var onChange: SheetChange
    public var offChange: SheetChange

    public init(title: String, visible: Bool, enabled: Bool, isOn: Bool, onChange: SheetChange, offChange: SheetChange) {
        self.title = title
        self.visible = visible
        self.enabled = enabled
        self.isOn = isOn
        self.onChange = onChange
        self.offChange = offChange
    }

    public func change(_ on: Bool) -> SheetChange { on ? onChange : offChange }
}
