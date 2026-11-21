import Foundation

struct Machine: Decodable {
    static let quietAfter: TimeInterval = 40

    let answering: Bool
    let heard: Date?
    let ssh: String?
    let cores: Int
    let load: Double
    let memory: Space
    let disk: Space
    let sessions: [Session]
    let pressure: Pressure?
    let swap: Space?
    let bootedAt: Date?
    let battery: Battery?
    let critical: Double?
    let relayed: Bool?
    let history: [Reading]?

    func answers(at now: Date) -> Bool {
        guard answering, let heard else { return false }
        return now.timeIntervalSince(heard) <= Machine.quietAfter
    }
}

struct Space: Decodable {
    let usedMB: Int
    let totalMB: Int

    var fraction: Double {
        Double(usedMB) / Double(totalMB)
    }
}

enum Pressure: String, Decodable {
    case normal, warning, critical
}

struct Battery: Decodable {
    let percent: Int
    let onBattery: Bool
}

struct Session: Decodable {
    let id: String
    let title: String
    let folder: String
    let state: String
    let waitingOn: String?
    let since: Date?
    let cpu: Int
    let memoryMB: Int
    let cpuHistory: [CPUReading]?
    let remoteControl: URL?
    let hostSession: String?
    let tmux: String?
    let rex: RexBlock?
    let app: String?
    let tty: String?
}

struct RexBlock: Decodable, Equatable {
    let session: String
    let block: String
}

struct Reading: Decodable {
    let at: TimeInterval
    let cpu: Int?
    let temp: Double?
    let thermal: Int?
    let rx: Int?
    let tx: Int?
}

struct CPUReading: Decodable {
    let at: TimeInterval
    let cpu: Int
}

struct Row: Identifiable {
    let machine: String
    let remote: String?
    let session: Session

    var id: String {
        "\(machine)/\(session.id)"
    }

    private var rank: Int {
        ["waiting", "busy"].firstIndex(of: session.state) ?? 2
    }

    static func urgent(_ rows: [Row]) -> [Row] {
        rows.sorted {
            guard $0.rank == $1.rank else { return $0.rank < $1.rank }
            guard $0.rank == 0 else { return ($0.session.title, $0.id) < ($1.session.title, $1.id) }
            return ($0.session.since ?? .distantFuture, $0.id) < ($1.session.since ?? .distantFuture, $1.id)
        }
    }

    static func ordered(_ rows: [Row], keeping order: [String]?) -> [Row] {
        guard let order else { return rows }
        let byID = Dictionary(uniqueKeysWithValues: rows.map { ($0.id, $0) })
        let kept = Set(order)
        return order.compactMap { byID[$0] } + rows.filter { !kept.contains($0.id) }
    }
}

extension JSONDecoder {
    static let magi = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }()
}
