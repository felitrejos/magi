import Foundation

struct Machine: Decodable {
    static let quietAfter: TimeInterval = 40

    let answering: Bool
    let heard: Date?
    let cores: Int
    let load: Double
    let memory: Space
    let disk: Space
    let sessions: [Session]

    func answers(at now: Date) -> Bool {
        guard answering, let heard else { return false }
        return now.timeIntervalSince(heard) <= Machine.quietAfter
    }
}

struct Space: Decodable {
    let usedMB: Int
    let totalMB: Int
}

struct Session: Decodable, Identifiable {
    let id: String
    let title: String
    let folder: String
    let state: String
    let waitingOn: String?
    let since: Date?
    let cpu: Int
    let memoryMB: Int
}

struct Move: Decodable {
    let time: Date
    let from: String
    let to: String
    let toDir: String
    let note: String?

    static func all(in data: Data) -> [Move] {
        data.split(separator: UInt8(ascii: "\n")).compactMap { try? JSONDecoder.magi.decode(Move.self, from: $0) }
    }
}

extension JSONDecoder {
    static let magi = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()
}
