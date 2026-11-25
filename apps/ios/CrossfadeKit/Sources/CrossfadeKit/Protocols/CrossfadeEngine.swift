import Foundation

/// Makes what the app needs from files: the seam the view models are written against
/// (`LiveEngine` over the C library; fakes in the tests). `inspect` and `makeSheet` read the
/// file, so call them off the main actor.
public protocol CrossfadeEngine: AnyObject {
    func about() throws -> About
    func inspect(path: String) throws -> InspectedFile
    func makeSheet(path: String) throws -> any SheetDriving
    func makeAnalysis(path: String) throws -> any AnalysisDriving
}
