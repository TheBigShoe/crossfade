import Foundation

/// A file's waveform and loudness analysis. `updates` ends after yielding a state that is not
/// `running`.
public protocol AnalysisDriving: AnyObject {
    func state() throws -> AnalysisState
    func columns(lane: Int, startFrame: UInt64, framesPerColumn: UInt64, count: Int) throws -> WaveColumns
    func cancel()
    func updates() -> AsyncStream<AnalysisState>
}
