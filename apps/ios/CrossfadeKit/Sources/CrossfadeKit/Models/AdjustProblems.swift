import Foundation

/// Why each Adjust row's text cannot be used, in the sheet's words.
public struct AdjustProblems: Codable, Equatable, Sendable {
    public var level: String?
    public var trim: String?
    public var fade: String?

    public init(
        level: String?,
        trim: String?,
        fade: String?
    ) {
        self.level = level
        self.trim = trim
        self.fade = fade
    }
}
