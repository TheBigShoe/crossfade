import UniformTypeIdentifiers

/// The file types the importer and drops offer: any audio, plus the Xiph types the
/// Info.plist declares (iOS may not know FLAC, Ogg or Opus as audio by itself).
enum AudioTypes {
    static let declared = ["org.xiph.flac", "org.xiph.ogg-audio", "org.xiph.opus"]

    static var importable: [UTType] {
        [UTType.audio] + declared.compactMap { UTType($0) }
    }
}
