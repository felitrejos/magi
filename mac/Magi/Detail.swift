import SwiftUI

struct Detail {
    let measure: Measure
    var scale: String?
    let columns: [[String]]
    let entries: [Entry]
    var parts: [Share] = []
}

struct Entry: Identifiable {
    let id: String
    let name: String
    var row: Row?
    let fraction: Double
    let figures: [String]
    let spoken: String
    var health = Health.fine
}

enum MemoryKind: String, CaseIterable {
    case app, wired, compressed, cached, free

    var title: String {
        switch self {
        case .app: "App memory"
        case .wired: "Wired"
        case .compressed: "Compressed"
        case .cached: "Cached"
        case .free: "Free"
        }
    }

    var style: AnyShapeStyle {
        switch self {
        case .app: AnyShapeStyle(.blue)
        case .wired: AnyShapeStyle(.orange)
        case .compressed: AnyShapeStyle(.purple)
        case .cached: AnyShapeStyle(.teal)
        case .free: AnyShapeStyle(.quaternary)
        }
    }
}

struct Share: Identifiable {
    let kind: MemoryKind
    let mb: Int

    var id: MemoryKind { kind }
}

extension Machine {
    var parts: [Share] {
        MemoryKind.allCases.compactMap { kind in memoryParts?.first { $0.kind == kind.rawValue }.map { Share(kind: kind, mb: $0.mb) } }
    }

    func hasDetail(_ module: Module) -> Bool {
        switch module {
        case .cpu: top?.cpu?.isEmpty == false
        case .memory: top?.memory?.isEmpty == false
        case .disk: volumes?.isEmpty == false
        case .network: interfaces?.isEmpty == false
        case .temperature: sensors?.isEmpty == false
        }
    }

    func detail(_ module: Module, on name: String) -> Detail? {
        guard hasDetail(module), let measure = measures([module]).first else { return nil }
        switch module {
        case .cpu:
            return Detail(measure: measure, scale: "% of one core", columns: [["999%"]], entries: (top?.cpu ?? []).map { program in
                Entry(id: "\(program.pid)", name: program.name, row: row(program.session, on: name), fraction: Double(program.cpu) / 100, figures: ["\(program.cpu)%"], spoken: "\(program.cpu) percent of one core")
            }.unique)
        case .memory:
            return Detail(measure: measure, scale: amount(memory).unit, columns: [["999 MB", amount(megabytes: memory.totalMB)]], entries: (top?.memory ?? []).map { program in
                Entry(id: "\(program.pid)", name: program.name, row: row(program.session, on: name), fraction: Double(program.memoryMB) / Double(memory.totalMB), figures: [amount(megabytes: program.memoryMB)], spoken: "\(amount(megabytes: program.memoryMB)) \(amount(memory).unit)")
            }.unique, parts: parts)
        case .disk:
            return Detail(measure: measure, columns: [["\(figure(99.9)) of \(figure(99.9)) GB"]], entries: (volumes ?? []).map { volume in
                let space = Space(usedMB: volume.usedMB, totalMB: volume.totalMB)
                let (used, total) = amount(space)
                return Entry(id: volume.mount, name: volume.name, fraction: space.fraction, figures: ["\(used) \(total)"], spoken: ["\(used) \(total) used, \(amount(megabytes: volume.totalMB - volume.usedMB, rounded: true)) free", space.health.alert(["nearly full", "full"])].compactMap(\.self).joined(separator: ", "), health: space.health)
            }.unique)
        case .network:
            let busiest = (interfaces ?? []).reduce(1) { max($0, $1.rx + $1.tx) }
            return Detail(measure: measure, columns: [["\u{2193} \(rate(99_900_000))"], ["\u{2191} \(rate(99_900_000))"]], entries: (interfaces ?? []).map { interface in
                let (down, up) = (rate(Double(interface.rx)), rate(Double(interface.tx)))
                return Entry(id: interface.name, name: interface.name, fraction: Double(interface.rx + interface.tx) / Double(busiest), figures: ["\u{2193} \(down)", "\u{2191} \(up)"], spoken: "\(down) down, \(up) up")
            }.unique)
        case .temperature:
            return Detail(measure: measure, columns: [[degrees(100)]], entries: (sensors ?? []).map { sensor in
                let health = Health(celsius: sensor.temp, critical: sensor.critical)
                return Entry(id: sensor.name, name: sensor.name, fraction: sensor.temp / (sensor.critical ?? 100), figures: [degrees(sensor.temp)], spoken: ["\(Int(sensor.temp.rounded())) degrees Celsius", sensor.critical.map { "critical at \(Int($0.rounded())) degrees" }, health.alert(["warm", "hot"])].compactMap(\.self).joined(separator: ", "), health: health)
            }.unique)
        }
    }

    private func row(_ session: String?, on name: String) -> Row? {
        session.flatMap { id in sessions.first { $0.id == id } }.map { Row(machine: name, remote: remote, session: $0) }
    }
}

struct DetailCaption: View {
    let measure: Measure

    var body: some View {
        let value = Text([measure.value, measure.unit].compactMap(\.self).joined(separator: measure.unit == "%" ? "" : " "))
            .fontWeight(.medium)
            .foregroundStyle(measure.health == .fine ? AnyShapeStyle(.primary) : AnyShapeStyle(measure.health.color))
        let phrases = [value] + measure.detail.components(separatedBy: " \u{00B7} ").map { Text($0) }
        Flow(spacing: 4) {
            ForEach(phrases.indices, id: \.self) { index in
                index == phrases.count - 1 ? phrases[index] : Text("\(phrases[index]) \u{00B7}")
            }
        }
        .foregroundStyle(.secondary)
        .font(.subheadline)
        .monospacedDigit()
        .accessibilityElement(children: .combine)
        .accessibilityValue(measure.alert ?? "")
    }
}

struct MemoryBar: View {
    let parts: [Share]

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            GeometryReader { proxy in
                let gap = 2.0
                let total = Double(parts.map(\.mb).reduce(0, +))
                let room = proxy.size.width - gap * Double(parts.count - 1)
                HStack(spacing: gap) {
                    ForEach(parts) { part in
                        Rectangle()
                            .fill(part.kind.style)
                            .frame(width: room * Double(part.mb) / total)
                    }
                }
            }
            .frame(height: 6)
            .clipShape(.capsule)
            .accessibilityHidden(true)
            Flow(spacing: 10) {
                ForEach(parts) { part in
                    HStack(spacing: 5) {
                        Circle()
                            .fill(part.kind.style)
                            .frame(width: 6, height: 6)
                        Text("\(part.kind.title) \(tenths(Double(part.mb) / 1024)) GB")
                    }
                    .accessibilityElement(children: .combine)
                }
            }
            .font(.footnote)
            .monospacedDigit()
            .foregroundStyle(.secondary)
        }
    }
}

struct Flow: Layout {
    let spacing: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout Void) -> CGSize {
        let frames = place(subviews, width: proposal.width ?? .infinity)
        return CGSize(width: proposal.width ?? frames.map(\.maxX).max() ?? 0, height: frames.map(\.maxY).max() ?? 0)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout Void) {
        for (subview, frame) in zip(subviews, place(subviews, width: bounds.width)) {
            subview.place(at: CGPoint(x: bounds.minX + frame.minX, y: bounds.minY + frame.minY), proposal: ProposedViewSize(frame.size))
        }
    }

    private func place(_ subviews: Subviews, width: CGFloat) -> [CGRect] {
        var frames: [CGRect] = []
        var origin = CGPoint.zero
        var line: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if origin.x > 0, origin.x + size.width > width {
                origin = CGPoint(x: 0, y: origin.y + line + 2)
                line = 0
            }
            frames.append(CGRect(origin: origin, size: size))
            origin.x += size.width + spacing
            line = max(line, size.height)
        }
        return frames
    }
}

struct DetailList: View {
    let detail: Detail
    let entries: [Entry]
    @Binding var highlight: String?
    let open: (Route) -> Void

    var body: some View {
        VStack(spacing: 0) {
            section(detail.measure.module.heading, scale: detail.scale)
            ForEach(entries) { entry in
                DetailRow(entry: entry, columns: detail.columns, highlighted: highlight == entry.id, open: open)
                    .id(entry.id)
                    .highlights(entry.id, in: $highlight, when: entry.row?.route != nil)
            }
        }
    }
}

struct DetailRow: View {
    let entry: Entry
    let columns: [[String]]
    let highlighted: Bool
    let open: (Route) -> Void
    @Environment(\.accessibilityDifferentiateWithoutColor) private var differentiate

    var body: some View {
        let route = entry.row?.route
        VStack(alignment: .leading, spacing: 1) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(entry.name)
                    .font(.body)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: .infinity, alignment: .leading)
                HStack(spacing: 6) {
                    if let route {
                        HostIcon(host: route.host)
                    } else if differentiate, entry.health != .fine {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(entry.health.color)
                            .frame(width: 16, height: 16)
                            .accessibilityHidden(true)
                    }
                    Level(fraction: entry.fraction, tint: entry.health == .fine ? Color(nsColor: .secondaryLabelColor) : entry.health.color, minimum: 0)
                        .frame(width: 44)
                        .animation(.easeOut(duration: 0.2), value: entry.fraction)
                    ForEach(entry.figures.indices, id: \.self) { index in
                        ZStack(alignment: .trailing) {
                            ForEach(columns[index].indices, id: \.self) { widest in
                                Text(columns[index][widest])
                                    .hidden()
                            }
                            Text(entry.figures[index])
                                .foregroundStyle(entry.health == .fine ? AnyShapeStyle(.primary) : AnyShapeStyle(entry.health.color))
                                .contentTransition(.opacity)
                        }
                    }
                    .font(.subheadline)
                    .monospacedDigit()
                    .animation(.easeOut(duration: 0.2), value: entry.figures)
                }
                .fixedSize()
            }
            if let row = entry.row {
                Text(row.session.title)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .choice(route, highlighted: highlighted, height: entry.row == nil ? 32 : 44, label: [entry.name, entry.row?.session.title].compactMap(\.self).joined(separator: ", "), value: entry.spoken, open: open)
    }
}
