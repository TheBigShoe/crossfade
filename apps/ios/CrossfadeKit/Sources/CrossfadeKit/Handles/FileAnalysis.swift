import CrossfadeFFI
import Foundation

/// Owns a `CfAnalysis`: the waveform and loudness of one file, worked out on the library's
/// thread. Destroying it cancels it without waiting.
public final class FileAnalysis: AnalysisDriving, @unchecked Sendable {
    let handle: OpaquePointer

    /// Starts analysing. An unreadable file still gives an analysis, whose state ends `failed`.
    public init(path: String) throws {
        handle = try FFI.handle { out in path.withCString { cf_analysis_new($0, out) } }
    }

    deinit {
        cf_analysis_destroy(handle)
    }

    public func state() throws -> AnalysisState {
        try FFI.json(AnalysisState.self) { cf_analysis_state(handle, $0) }
    }

    /// `count` columns of `lane`, each `framesPerColumn` frames wide from `startFrame`.
    public func columns(lane: Int, startFrame: UInt64, framesPerColumn: UInt64, count: Int) throws -> WaveColumns {
        guard count > 0, lane >= 0 else { return .empty }
        var mins = [Float](repeating: .nan, count: count)
        var maxs = [Float](repeating: .nan, count: count)
        var written: Int = 0
        let status: Int32 = mins.withUnsafeMutableBufferPointer { lo in
            maxs.withUnsafeMutableBufferPointer { hi in
                cf_analysis_columns(
                    handle, UInt32(lane), startFrame, max(framesPerColumn, 1),
                    lo.baseAddress, hi.baseAddress, count, &written)
            }
        }
        try FFI.check(status)
        return WaveColumns(mins: mins, maxs: maxs, written: written)
    }

    public func cancel() {
        _ = cf_analysis_cancel(handle)
    }

    /// The state now and on every change, ending after the first state that is not `running`.
    public func updates() -> AsyncStream<AnalysisState> {
        RevisionStream.make(
            read: { try self.state() },
            revision: { $0.revision },
            wait: { since in
                var now: UInt64 = 0
                _ = cf_analysis_wait(self.handle, since, RevisionStream.waitMilliseconds, &now)
            },
            isFinal: { $0.state != "running" })
    }
}
