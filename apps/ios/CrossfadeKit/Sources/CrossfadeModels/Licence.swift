import Foundation

/// A licence text the app carries, read from a file in the app's bundle.
public struct Licence: Identifiable, Equatable, Sendable {
    public var id: String { resource }
    public var title: String
    public var detail: String
    /// The bundle resource's name (no extension).
    public var resource: String

    public init(title: String, detail: String, resource: String) {
        self.title = title
        self.detail = detail
        self.resource = resource
    }

    /// What the app must carry: the workspace's two licences, the Opus codec's BSD notice and
    /// the notices of the third-party crates linked into the library.
    public static let carried: [Licence] = [
        Licence(title: "MIT License", detail: "Crossfade", resource: "LICENSE-MIT"),
        Licence(title: "Apache License 2.0", detail: "Crossfade", resource: "LICENSE-APACHE"),
        Licence(title: "BSD 3-Clause", detail: "The Opus codec (crossfade-opus)", resource: "COPYING"),
        Licence(title: "Third-party notices", detail: "Rust crates in the library", resource: "ThirdPartyNotices"),
    ]

    /// The text from `bundle`, or a line saying it is missing from this build.
    public func text(in bundle: Bundle) -> String {
        let url = bundle.url(forResource: resource, withExtension: nil)
            ?? bundle.url(forResource: resource, withExtension: "txt")
        guard let url = url, let text = try? String(contentsOf: url, encoding: .utf8) else {
            return "This build does not include \(resource)."
        }
        return text
    }
}
