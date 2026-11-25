import CrossfadeKit
import Foundation
import Observation

/// About Crossfade: the library's own lines, and the licences the app carries.
@MainActor
@Observable
public final class AboutModel {
    public private(set) var lines: [String] = []
    public private(set) var version: String = ""
    public let licences: [Licence] = Licence.carried

    public init() {}

    public func load(engine: any CrossfadeEngine) {
        do {
            let about = try engine.about()
            lines = about.lines
            version = about.version
        } catch {
            lines = [error.localizedDescription]
        }
    }
}
