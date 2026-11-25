import Foundation

/// `cf_sheet_state`: the convert sheet, one entry per row with what it holds, offers, shows
/// and enables. The app renders this and never works out for itself what is valid.
public struct SheetState: Codable, Equatable, Sendable {
    public var input: String
    public var output: String
    public var overwrite: Bool
    public var preset: PresetRow
    public var container: ContainerRow
    public var format: FormatRow
    public var compressionLevel: LevelRow
    public var bitrate: BitrateRow
    public var copy: CopyRow
    public var sampleRate: SampleRateRow
    public var channels: ChannelsRow
    public var adjust: AdjustState
    /// The line under Adjust: what will be written, or why the controls are off.
    public var processingNote: String?
    /// The plan's assumptions, the ones the CLI prints.
    public var notes: [String]
    public var problems: [SheetProblem]
    /// The equivalent `crossfade convert` line; null while the choices have a problem.
    public var commandLine: String?
    public var canQueue: Bool
    public var request: RequestState

    public init(
        input: String, output: String, overwrite: Bool, preset: PresetRow,
        container: ContainerRow, format: FormatRow, compressionLevel: LevelRow,
        bitrate: BitrateRow, copy: CopyRow, sampleRate: SampleRateRow, channels: ChannelsRow,
        adjust: AdjustState, processingNote: String?, notes: [String],
        problems: [SheetProblem], commandLine: String?, canQueue: Bool, request: RequestState
    ) {
        self.input = input
        self.output = output
        self.overwrite = overwrite
        self.preset = preset
        self.container = container
        self.format = format
        self.compressionLevel = compressionLevel
        self.bitrate = bitrate
        self.copy = copy
        self.sampleRate = sampleRate
        self.channels = channels
        self.adjust = adjust
        self.processingNote = processingNote
        self.notes = notes
        self.problems = problems
        self.commandLine = commandLine
        self.canQueue = canQueue
        self.request = request
    }
}
