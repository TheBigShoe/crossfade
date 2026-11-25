import CrossfadeFFI
import Foundation

/// The library's version and the start-up check of its interface version.
public enum CrossfadeLibrary {
    /// The interface version these bindings were written for (crossfade.h's CF_ABI_VERSION).
    public static let expectedABIVersion: UInt32 = 1

    public static var abiVersion: UInt32 { cf_abi_version() }

    /// The library's version, such as "0.0.0". A static string the library keeps.
    public static var version: String {
        let pointer: UnsafePointer<CChar>? = cf_version()
        guard let pointer = pointer else { return "" }
        return String(cString: pointer)
    }

    /// Throws unless the linked library speaks the interface these bindings expect: both the
    /// header the module was compiled against and the library linked in must say 1.
    public static func checkABI() throws {
        let header = UInt32(CF_ABI_VERSION)
        let linked = cf_abi_version()
        if header != expectedABIVersion || linked != expectedABIVersion {
            throw CrossfadeError(
                code: CrossfadeError.incompatibleCode,
                message: "This app was built for version \(expectedABIVersion) of Crossfade's "
                    + "library interface, but the library speaks version \(linked) "
                    + "(its header says \(header)). Rebuild the framework with `just ios-framework`.")
        }
    }
}
