import Foundation

/// `cf_about`: the About box's lines, the library's version and interface version.
public struct About: Codable, Equatable, Sendable {
    public var lines: [String]
    public var version: String
    public var abiVersion: Int

    public init(
        lines: [String],
        version: String,
        abiVersion: Int
    ) {
        self.lines = lines
        self.version = version
        self.abiVersion = abiVersion
    }
}
