import CrossfadeKit
import Foundation

/// The Adjust section of the convert form: level, trim and fade, from the sheet's state.
public struct AdjustForm: Equatable, Sendable {
    public var open: Bool
    public var enabled: Bool
    public var summary: String?
    /// Off, Gain, Normalize: the level modes crossfade-ffi's README names.
    public var levelModes: [Choice]
    public var levelModeIndex: Int?
    /// Shown while the level mode is not off.
    public var level: TextRow?
    public var trimStart: TextRow
    public var trimEnd: TextRow
    public var fadeIn: TextRow
    public var fadeOut: TextRow

    public static let modes: [(id: String, label: String)] = [
        ("off", "Off"), ("gain", "Gain"), ("normalize", "Normalize"),
    ]

    public init(state a: AdjustState) {
        open = a.open
        enabled = a.enabled
        summary = a.summary
        levelModes = Self.modes.map { Choice(label: $0.label, change: .levelMode($0.id)) }
        levelModeIndex = Self.modes.firstIndex { $0.id == a.levelMode }
        switch a.levelMode {
        case "gain":
            level = TextRow(
                title: "Gain", text: a.levelValue, placeholder: "-3", unit: "dB",
                problem: a.problems.level, send: .levelValue)
        case "normalize":
            level = TextRow(
                title: "Target", text: a.levelValue, placeholder: "-16", unit: "LUFS",
                problem: a.problems.level, send: .levelValue)
        default:
            level = nil
        }
        trimStart = TextRow(
            title: "Start", text: a.trimStart, placeholder: "0:00", unit: "",
            problem: a.problems.trim, send: .trimStart)
        trimEnd = TextRow(
            title: "End", text: a.trimEnd, placeholder: "End of file", unit: "",
            problem: nil, send: .trimEnd)
        fadeIn = TextRow(
            title: "Fade in", text: a.fadeIn, placeholder: "0", unit: "s",
            problem: a.problems.fade, send: .fadeIn)
        fadeOut = TextRow(
            title: "Fade out", text: a.fadeOut, placeholder: "0", unit: "s",
            problem: nil, send: .fadeOut)
    }

    /// What opening or closing the section sends.
    public func openChange(_ open: Bool) -> SheetChange { .adjustOpen(open) }
}
