import CrossfadeKit
import Foundation

/// An engine that answers from tables: a file per path, sheets from a factory.
final class FakeEngine: CrossfadeEngine {
    var files: [String: InspectedFile] = [:]
    var makeSheetResult: () throws -> any SheetDriving = {
        throw CrossfadeError(code: 4, message: "no sheet")
    }
    private(set) var inspected: [String] = []

    func about() throws -> About {
        About(lines: ["Crossfade test"], version: "0.0.0", abiVersion: 1)
    }

    func inspect(path: String) throws -> InspectedFile {
        inspected.append(path)
        if let file = files[path] { return file }
        return InspectedFile(
            path: path, name: (path as NSString).lastPathComponent, readable: false,
            problem: "unrecognised format", summary: nil, sections: [])
    }

    func makeSheet(path: String) throws -> any SheetDriving {
        try makeSheetResult()
    }

    func makeAnalysis(path: String) throws -> any AnalysisDriving {
        throw CrossfadeError(code: 7, message: "no analysis in the fake")
    }

    static func readable(_ path: String, summary: String = "FLAC · 44.1 kHz · stereo · 16-bit · 0:01.000") -> InspectedFile {
        InspectedFile(
            path: path, name: (path as NSString).lastPathComponent, readable: true,
            problem: nil, summary: summary, sections: [])
    }
}
