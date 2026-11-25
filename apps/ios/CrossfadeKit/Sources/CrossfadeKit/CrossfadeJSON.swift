import Foundation

/// How every document from the library is read: snake_case keys to camelCase properties, and
/// unknown keys ignored (Codable's default), since new keys do not change the ABI version.
public enum CrossfadeJSON {
    public static func decode<T: Decodable>(_ type: T.Type, from text: String) throws -> T {
        try decode(type, from: Data(text.utf8))
    }

    public static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do {
            return try decoder.decode(type, from: data)
        } catch {
            throw CrossfadeError(
                code: CrossfadeError.decodingCode,
                message: "Could not read Crossfade's \(String(describing: type)): \(error)")
        }
    }
}
