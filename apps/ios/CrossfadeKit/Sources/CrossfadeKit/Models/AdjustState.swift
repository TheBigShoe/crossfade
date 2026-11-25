import Foundation

/// The Adjust section: level (`off gain normalize`), trim and fade, as typed.
public struct AdjustState: Codable, Equatable, Sendable {
    public var open: Bool
    public var enabled: Bool
    public var summary: String?
    public var levelMode: String
    public var levelValue: String
    public var trimStart: String
    public var trimEnd: String
    public var fadeIn: String
    public var fadeOut: String
    public var problems: AdjustProblems

    public init(
        open: Bool,
        enabled: Bool,
        summary: String?,
        levelMode: String,
        levelValue: String,
        trimStart: String,
        trimEnd: String,
        fadeIn: String,
        fadeOut: String,
        problems: AdjustProblems
    ) {
        self.open = open
        self.enabled = enabled
        self.summary = summary
        self.levelMode = levelMode
        self.levelValue = levelValue
        self.trimStart = trimStart
        self.trimEnd = trimEnd
        self.fadeIn = fadeIn
        self.fadeOut = fadeOut
        self.problems = problems
    }
}
