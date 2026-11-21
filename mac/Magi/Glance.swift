import Foundation
import UserNotifications

struct Glance: Equatable {
    let waiting: [String: Waiting]
    let answering: Set<String>
    let hasMachines: Bool

    var symbol: String {
        waiting.isEmpty ? answering.isEmpty ? "orbit.slash" : "orbit" : "orbit.badge"
    }

    var label: String {
        if !waiting.isEmpty { return "magi, \(waiting.count) waiting on you" }
        if !hasMachines { return "magi, not set up" }
        return answering.isEmpty ? "magi, no machine is answering" : "magi"
    }

    func started(after shown: Glance?) -> [String: Waiting] {
        guard let shown else { return [:] }
        return waiting.filter { shown.waiting[$0.key] == nil }
    }

    func stopped(after shown: Glance?) -> [String] {
        shown?.waiting.keys.filter { waiting[$0] == nil } ?? []
    }

    func stale(_ delivered: [String]) -> [String] {
        delivered.filter { waiting[$0] == nil }
    }
}

extension Glance {
    @MainActor init(store: Store) {
        self.init(waiting: Dictionary(uniqueKeysWithValues: store.rows.filter { $0.session.state == "waiting" }.map { ($0.id, Waiting(row: $0)) }), answering: store.answering, hasMachines: !store.machines.isEmpty)
    }
}

struct Waiting: Equatable {
    let title: String
    let machine: String
    let folder: String
    let on: String?

    var announcement: String {
        "\(title) on \(machine) is waiting on you"
    }

    var content: UNNotificationContent {
        let content = UNMutableNotificationContent()
        content.title = title
        content.subtitle = "\(machine) \u{00B7} \(folder)"
        content.body = on ?? "Waiting on you."
        return content
    }
}

extension Waiting {
    init(row: Row) {
        self.init(title: row.session.title, machine: row.machine, folder: URL(filePath: row.session.folder).lastPathComponent, on: row.session.waitingOn)
    }
}
