import Foundation

/// A picker row of the convert form, exactly as the sheet's state describes it.
public struct ChoiceRow: Equatable, Sendable {
    public var title: String
    public var visible: Bool
    public var enabled: Bool
    public var choices: [Choice]
    /// The index of the choice the sheet holds; nil when it holds none of them.
    public var selected: Int?
    /// What the row reads when nothing offered matches (the sheet's own label).
    public var label: String
    /// A line under the row, from the sheet.
    public var hint: String?

    public init(
        title: String, visible: Bool, enabled: Bool, choices: [Choice], selected: Int?,
        label: String, hint: String? = nil
    ) {
        self.title = title
        self.visible = visible
        self.enabled = enabled
        self.choices = choices
        self.selected = selected
        self.label = label
        self.hint = hint
    }
}
