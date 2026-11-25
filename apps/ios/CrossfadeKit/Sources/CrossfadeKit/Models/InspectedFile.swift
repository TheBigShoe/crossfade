import Foundation

/// `cf_inspect`: what a file is. An unreadable file has `readable` false and a `problem`.
public struct InspectedFile: Codable, Equatable, Sendable {
    public var path: String
    public var name: String
    public var readable: Bool
    public var problem: String?
    public var summary: String?
    public var sections: [InspectorSection]

    public init(
        path: String,
        name: String,
        readable: Bool,
        problem: String?,
        summary: String?,
        sections: [InspectorSection]
    ) {
        self.path = path
        self.name = name
        self.readable = readable
        self.problem = problem
        self.summary = summary
        self.sections = sections
    }
}
