import SwiftUI

struct Detail {
    let measure: Measure
    let heading: String
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

    func detail(_ module: Module, on name: String) -> Detail? {
        guard let measure = measures([module]).first else { return nil }
        let detail: Detail
        switch module {
        case .cpu:
            detail = Detail(measure: measure, heading: "Processes", scale: "% of one core", columns: [["\(cores * 100)%"]], entries: (top?.cpu ?? []).map { program in
                Entry(id: "\(program.pid)", name: program.name, row: row(program, on: name), fraction: Double(program.cpu) / 100, figures: ["\(program.cpu)%"], spoken: "\(program.cpu) percent of one core")
            })
        case .memory:
            detail = Detail(measure: measure, heading: "Programs", scale: amount(memory).unit, columns: [["999 MB", amount(megabytes: memory.totalMB)]], entries: (top?.memory ?? []).map { program in
                Entry(id: "\(program.pid)", name: program.name, row: row(program, on: name), fraction: Double(program.memoryMB) / Double(memory.totalMB), figures: [amount(megabytes: program.memoryMB)], spoken: "\(amount(megabytes: program.memoryMB)) \(amount(memory).unit)")
            }, parts: parts)
        case .disk:
            detail = Detail(measure: measure, heading: "Volumes", columns: [["\(figure(99.9)) of \(figure(99.9)) GB"]], entries: (volumes ?? []).map { volume in
                let space = Space(usedMB: volume.usedMB, totalMB: volume.totalMB)
                let (used, total) = amount(space)
                return Entry(id: volume.mount, name: volume.name, fraction: space.fraction, figures: ["\(used) \(total)"], spoken: "\(used) \(total) used, \(amount(megabytes: volume.totalMB - volume.usedMB, rounded: true)) free", health: space.health)
            })
        case .network:
            let busiest = (interfaces ?? []).reduce(1) { max($0, $1.rx + $1.tx) }
            detail = Detail(measure: measure, heading: "Interfaces", columns: [["\u{2193} \(rate(99_900_000))"], ["\u{2191} \(rate(99_900_000))"]], entries: (interfaces ?? []).map { interface in
                let (down, up) = (rate(Double(interface.rx)), rate(Double(interface.tx)))
                return Entry(id: interface.name, name: interface.name, fraction: Double(interface.rx + interface.tx) / Double(busiest), figures: ["\u{2193} \(down)", "\u{2191} \(up)"], spoken: "\(down) down, \(up) up")
            })
        case .temperature:
            detail = Detail(measure: measure, heading: "Sensors", columns: [[degrees(100)]], entries: (sensors ?? []).enumerated().map { index, sensor in
                Entry(id: "\(index)", name: sensor.name, fraction: sensor.temp / (sensor.critical ?? 100), figures: [degrees(sensor.temp)], spoken: "\(Int(sensor.temp.rounded())) degrees Celsius" + (sensor.critical.map { ", critical at \(Int($0)) degrees" } ?? ""), health: Health(celsius: sensor.temp, critical: sensor.critical))
            })
        }
        return detail.entries.isEmpty ? nil : detail
    }

    private func row(_ program: Program, on name: String) -> Row? {
        program.session.flatMap { id in sessions.first { $0.id == id } }.map { Row(machine: name, remote: remote, session: $0) }
    }
}

struct DetailCaption: View {
    let measure: Measure

    var body: some View {
        let value = Text([measure.value, measure.unit].compactMap(\.self).joined(separator: measure.unit == "%" ? "" : " "))
            .fontWeight(.medium)
            .foregroundStyle(measure.health == .fine ? AnyShapeStyle(.primary) : AnyShapeStyle(measure.health.color))
        Text("\(value) \u{00B7} \(measure.detail)")
            .foregroundStyle(.secondary)
            .font(.subheadline)
            .monospacedDigit()
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
            Legend(spacing: 10) {
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

struct Legend: Layout {
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
            HStack(alignment: .firstTextBaseline) {
                Text(detail.heading)
                    .font(.footnote.weight(.semibold))
                    .accessibilityAddTraits(.isHeader)
                Spacer(minLength: 8)
                if let scale = detail.scale {
                    Text(scale)
                        .font(.footnote)
                }
            }
            .foregroundStyle(.secondary)
            .padding(.horizontal, 8)
            .padding(.bottom, 2)
            ForEach(entries) { entry in
                DetailRow(entry: entry, columns: detail.columns, highlighted: highlight == entry.id, open: open)
                    .id(entry.id)
                    .onHover { inside in
                        if inside, entry.row?.route != nil {
                            highlight = entry.id
                        } else if highlight == entry.id {
                            highlight = nil
                        }
                    }
            }
        }
    }
}

struct DetailRow: View {
    let entry: Entry
    let columns: [[String]]
    let highlighted: Bool
    let open: (Route) -> Void

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
                    }
                    Level(fraction: entry.fraction, tint: entry.health == .fine ? Color(nsColor: .secondaryLabelColor) : entry.health.color, minimum: 0)
                        .frame(width: 44)
                        .animation(.easeOut(duration: 0.2), value: entry.fraction)
                    ForEach(entry.figures.indices, id: \.self) { index in
                        ZStack(alignment: .trailing) {
                            ForEach(columns[index], id: \.self) { widest in
                                Text(widest)
                                    .hidden()
                            }
                            Text(entry.figures[index])
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
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .frame(minHeight: entry.row == nil ? 32 : 44)
        .background {
            if highlighted {
                ConcentricRectangle(corners: .concentric(minimum: 8), isUniform: true)
                    .fill(.quaternary)
            }
        }
        .contentShape(.rect)
        .onTapGesture {
            route.map(open)
        }
        .help(route.map { "Open in \($0.host.name)" } ?? "")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel([entry.name, entry.row?.session.title].compactMap(\.self).joined(separator: ", "))
        .accessibilityValue(entry.spoken)
        .accessibilityAddTraits(route == nil ? [] : .isButton)
        .accessibilityActions {
            if let route {
                Button("Open in \(route.host.name)") { open(route) }
            }
        }
    }
}
