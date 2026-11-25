import Foundation

/// Something whose security scope can be entered and left: a URL from the file importer, a
/// document opened in place or a resolved bookmark. A protocol so the tests can count calls.
public protocol ScopedResource {
    func startAccessingSecurityScopedResource() -> Bool
    func stopAccessingSecurityScopedResource()
}

extension URL: ScopedResource {}
