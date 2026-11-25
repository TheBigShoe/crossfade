import Foundation

/// Waveform columns of one lane: a minimum and a maximum per column (linear, -1 to 1).
/// Columns past what has been analysed are NaN; `written` counts those with data.
public struct WaveColumns: Equatable, Sendable {
    public var mins: [Float]
    public var maxs: [Float]
    public var written: Int

    public init(mins: [Float], maxs: [Float], written: Int) {
        self.mins = mins
        self.maxs = maxs
        self.written = written
    }

    public static let empty = WaveColumns(mins: [], maxs: [], written: 0)
}
