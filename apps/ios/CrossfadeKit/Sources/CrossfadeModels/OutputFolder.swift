import Foundation

/// Where conversions are written: the app's Documents folder, which the Files app shows as
/// "On My iPhone › Crossfade" (the Info.plist turns on file sharing). It is inside the app's
/// container, so the library can write there without a security scope.
public struct OutputFolder: Equatable, Sendable {
    public let url: URL

    public init(url: URL) {
        self.url = url
    }

    public static func documents(fileManager: FileManager = .default) -> OutputFolder {
        let url = fileManager.urls(for: .documentDirectory, in: .userDomainMask)[0]
        return OutputFolder(url: url)
    }

    /// The path for a file of this name in the folder.
    public func path(forName name: String) -> String {
        url.appendingPathComponent(name).path(percentEncoded: false)
    }

    /// Whether a file name is a conversion's temporary file (`.{name}.{pid}.tmp` or
    /// `.{name}.{pid}-{serial}.tmp`, crossfade-app's `FsIo`). One is left behind only when
    /// the system ends the app mid-job.
    public static func isStaleTemporary(name: String) -> Bool {
        guard name.hasPrefix("."), name.hasSuffix(".tmp"), name.count > 5 else { return false }
        let core = name.dropFirst().dropLast(4)
        guard let dot = core.lastIndex(of: "."), dot != core.startIndex else { return false }
        let tail = core[core.index(after: dot)...]
        let parts = tail.split(separator: "-", omittingEmptySubsequences: false)
        guard parts.count == 1 || parts.count == 2 else { return false }
        return parts.allSatisfy { !$0.isEmpty && $0.allSatisfy { $0.isASCII && $0.isNumber } }
    }

    /// Removes temporary files a previous run left behind. Call at launch, before any job runs.
    @discardableResult
    public func sweepStaleTemporaryFiles(fileManager: FileManager = .default) -> [String] {
        let names = (try? fileManager.contentsOfDirectory(atPath: url.path(percentEncoded: false))) ?? []
        var removed: [String] = []
        for name in names where Self.isStaleTemporary(name: name) {
            if (try? fileManager.removeItem(at: url.appendingPathComponent(name))) != nil {
                removed.append(name)
            }
        }
        return removed
    }
}
