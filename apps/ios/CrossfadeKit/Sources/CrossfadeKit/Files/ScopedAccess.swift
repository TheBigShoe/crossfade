import Foundation

/// Holds a file's security scope from creation until `end()` or deinit, whichever comes first,
/// and stops it only if starting it succeeded (a file in the app's own container has no scope
/// to start). How long each kind of work needs it (crossfade-ffi's README):
/// - inspecting and opening a sheet: the call (`during`);
/// - a queued job: until it is no longer pending or running (the queue opens the input when
///   the job starts, not when it is queued), and the output's folder for the whole job;
/// - an analysis: until its state is no longer `running`;
/// - the player: for as long as the file is loaded.
public final class ScopedAccess {
    private let resource: any ScopedResource
    private var active: Bool

    public init(_ resource: any ScopedResource) {
        self.resource = resource
        active = resource.startAccessingSecurityScopedResource()
    }

    deinit {
        end()
    }

    /// Whether the scope was entered and has not been left yet.
    public var isActive: Bool { active }

    /// Leaves the scope; does nothing the second time.
    public func end() {
        if active {
            active = false
            resource.stopAccessingSecurityScopedResource()
        }
    }

    /// Runs `body` inside the scope, which ends however `body` returns.
    public static func during<T>(_ resource: any ScopedResource, _ body: () throws -> T) rethrows -> T {
        let access = ScopedAccess(resource)
        defer { access.end() }
        return try body()
    }
}
