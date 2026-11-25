import CrossfadeKit
import Foundation

/// The convert form, translated row for row from the sheet's state: what each row shows,
/// whether it is visible and enabled, what it offers and what choosing an option sends. Every
/// decision here is the sheet's; the only words added are row titles and units.
public struct ConvertForm: Equatable, Sendable {
    public var preset: ChoiceRow
    public var container: ChoiceRow
    public var format: ChoiceRow
    public var compression: ChoiceRow
    public var bitrate: ChoiceRow
    public var sampleRate: ChoiceRow
    public var channels: ChoiceRow
    public var copy: ToggleRow
    public var overwrite: ToggleRow
    public var adjust: AdjustForm
    /// The output's file name, and the whole path.
    public var outputName: String
    public var outputPath: String
    public var processingNote: String?
    public var notes: [String]
    /// The problems that belong to no Adjust row, in the sheet's words.
    public var problems: [String]
    public var commandLine: String?
    public var canQueue: Bool

    public init(state s: SheetState) {
        preset = ChoiceRow(
            title: "Preset", visible: true, enabled: true,
            choices: s.preset.options.map { Choice(label: $0.label, change: .preset($0.id)) },
            selected: s.preset.options.firstIndex { $0.id == s.preset.value },
            label: s.preset.options.first { $0.id == s.preset.value }?.label ?? "Custom")
        container = ChoiceRow(
            title: "Container", visible: true, enabled: true,
            choices: s.container.options.map { Choice(label: $0.label, change: .container($0.id)) },
            selected: s.container.options.firstIndex { $0.id == s.container.value },
            label: s.container.label)
        format = ChoiceRow(
            title: "Format", visible: s.format.visible, enabled: s.format.enabled,
            choices: s.format.options.map { Choice(label: $0.label, change: .format($0.id)) },
            selected: s.format.options.firstIndex { $0.id == s.format.value },
            label: s.format.label, hint: s.format.hint)
        compression = ChoiceRow(
            title: "Compression", visible: s.compressionLevel.visible,
            enabled: s.compressionLevel.enabled,
            choices: s.compressionLevel.options.map { level in
                Choice(label: level.map { String($0) } ?? "Default", change: .compressionLevel(level))
            },
            selected: s.compressionLevel.options.firstIndex { $0 == s.compressionLevel.value },
            label: s.compressionLevel.value.map { String($0) } ?? "Default")
        bitrate = ChoiceRow(
            title: "Bitrate", visible: s.bitrate.visible, enabled: s.bitrate.enabled,
            choices: s.bitrate.options.map { Choice(label: $0.label, change: .bitrate($0.value)) },
            selected: s.bitrate.options.firstIndex { $0.value == s.bitrate.value },
            label: s.bitrate.label)
        sampleRate = ChoiceRow(
            title: "Sample rate", visible: true, enabled: s.sampleRate.enabled,
            choices: s.sampleRate.options.map { Choice(label: $0.label, change: .sampleRate($0.value)) },
            selected: s.sampleRate.options.firstIndex { $0.value == s.sampleRate.value },
            label: s.sampleRate.label)
        channels = ChoiceRow(
            title: "Channels", visible: true, enabled: s.channels.enabled,
            choices: s.channels.options.map { Choice(label: $0.label, change: .channels($0.value)) },
            selected: s.channels.options.firstIndex { $0.value == s.channels.value },
            label: s.channels.label)
        copy = ToggleRow(
            title: "Copy audio unchanged", visible: s.copy.available, enabled: s.copy.available,
            isOn: s.copy.value, onChange: .copy(true), offChange: .copy(false))
        overwrite = ToggleRow(
            title: "Replace existing file", visible: true, enabled: true,
            isOn: s.overwrite, onChange: .overwrite(true), offChange: .overwrite(false))
        adjust = AdjustForm(state: s.adjust)
        outputPath = s.output
        outputName = (s.output as NSString).lastPathComponent
        processingNote = s.processingNote
        notes = s.notes
        problems = s.problems.filter { $0.kind != "adjust" }.map { $0.message }
        commandLine = s.commandLine
        canQueue = s.canQueue
    }
}
