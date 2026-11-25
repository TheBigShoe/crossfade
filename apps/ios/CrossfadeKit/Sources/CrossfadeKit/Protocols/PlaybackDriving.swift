import Foundation

/// The preview player, driven from the main actor only (the library ties a player to the
/// thread that made it).
@MainActor
public protocol PlaybackDriving: AnyObject {
    var state: PlayerState? { get }
    /// Called on the main actor whenever `state` changes.
    var onStateChange: ((PlayerState) -> Void)? { get set }
    func load(path: String) throws
    func play() throws
    func pause() throws
    func toggle() throws
    func stop() throws
    func seek(frame: UInt64) throws
    func scrub(frame: UInt64) throws
    func endScrub() throws
    /// The source frame the device has played; cheap enough for every display frame.
    func position() -> UInt64
}
