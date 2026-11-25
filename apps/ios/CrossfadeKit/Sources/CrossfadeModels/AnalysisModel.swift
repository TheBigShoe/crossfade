import CrossfadeKit
import Foundation
import Observation

/// One file's waveform and loudness while it is shown: follows the analysis's revisions,
/// holds the file's security scope until the analysis stops reading, and answers the
/// waveform's requests for columns.
@MainActor
@Observable
public final class AnalysisModel {
    public private(set) var state: AnalysisState?
    /// Why the analysis could not be started at all.
    public private(set) var failure: String?

    @ObservationIgnored private var analysis: (any AnalysisDriving)?
    @ObservationIgnored private var access: ScopedAccess?
    @ObservationIgnored private var watcher: Task<Void, Never>?

    public init() {}

    /// Starts analysing `url`, holding its scope until the analysis stops reading.
    public func start(engine: any CrossfadeEngine, url: URL) {
        stop()
        let access = ScopedAccess(url)
        do {
            let analysis = try engine.makeAnalysis(path: url.path(percentEncoded: false))
            self.access = access
            watch(analysis)
        } catch {
            access.end()
            failure = error.localizedDescription
        }
    }

    /// Follows `analysis` until it stops running.
    public func watch(_ analysis: any AnalysisDriving) {
        self.analysis = analysis
        let updates = analysis.updates()
        watcher = Task { [weak self] in
            for await next in updates {
                self?.apply(next)
            }
            self?.leaveScope()
        }
    }

    /// Cancels the analysis and leaves the file's scope.
    public func stop() {
        watcher?.cancel()
        watcher = nil
        analysis?.cancel()
        analysis = nil
        leaveScope()
    }

    /// Whether the file's scope is still held (for the tests).
    public var holdsScope: Bool { access?.isActive == true }

    public var isRunning: Bool { state?.state == "running" }

    /// The fraction analysed so far, while the total is known.
    public var progress: Double? {
        guard let state = state, state.state == "running", let total = state.frames, total > 0 else { return nil }
        return min(Double(state.framesDone) / Double(total), 1)
    }

    public var laneCount: Int { state?.lanes ?? 0 }

    public var totalFrames: UInt64 { state?.frames ?? state?.framesDone ?? 0 }

    /// The columns of every lane for the viewport at `columns` columns (a brief lock in the
    /// library); an empty lane where a lane cannot be read.
    public func lanes(viewport: WaveViewport, columns: Int) -> [WaveColumns] {
        guard let analysis = analysis, columns > 0 else { return [] }
        let start = viewport.startFrame
        let perColumn = viewport.framesPerColumn(columns: columns)
        return (0..<laneCount).map { lane in
            (try? analysis.columns(lane: lane, startFrame: start, framesPerColumn: perColumn, count: columns)) ?? .empty
        }
    }

    private func apply(_ next: AnalysisState) {
        state = next
        if next.state != "running" {
            // Done reading: the scope is no longer needed; the columns still are.
            leaveScope()
        }
    }

    private func leaveScope() {
        access?.end()
        access = nil
    }
}
