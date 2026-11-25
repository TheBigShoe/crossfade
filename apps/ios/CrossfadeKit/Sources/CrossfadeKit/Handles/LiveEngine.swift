import CrossfadeFFI
import Foundation

/// `CrossfadeEngine` over the C library. Holds no state; any thread may use it.
public final class LiveEngine: CrossfadeEngine, @unchecked Sendable {
    public init() {}

    public func about() throws -> About {
        try FFI.json(About.self) { cf_about($0) }
    }

    /// Probes the file's headers. An unreadable file is not an error: `readable` is false.
    public func inspect(path: String) throws -> InspectedFile {
        try FFI.json(InspectedFile.self) { out in path.withCString { cf_inspect($0, out) } }
    }

    public func makeSheet(path: String) throws -> any SheetDriving {
        try ConvertSheet(path: path)
    }

    public func makeAnalysis(path: String) throws -> any AnalysisDriving {
        try FileAnalysis(path: path)
    }
}
