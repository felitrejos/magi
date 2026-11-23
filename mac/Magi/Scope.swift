import SwiftUI

enum Module: String, CaseIterable, Identifiable {
    case cpu, memory, temperature, disk, network

    var id: Self { self }

    var title: String {
        switch self {
        case .cpu: "CPU"
        case .memory: "Memory"
        case .temperature: "Temperature"
        case .disk: "Disk"
        case .network: "Network"
        }
    }

    var subtitle: String {
        switch self {
        case .cpu: "Percent busy over the last hour, with load"
        case .memory: "Used of total, colored by memory pressure"
        case .temperature: "\u{00B0}C on Linux, thermal pressure on a Mac"
        case .disk: "Space used on the home volume"
        case .network: "Download and upload speed"
        }
    }

    var heading: String {
        switch self {
        case .cpu: "Processes"
        case .memory: "Programs"
        case .temperature: "Sensors"
        case .disk: "Volumes"
        case .network: "Interfaces"
        }
    }

    var symbol: String {
        switch self {
        case .cpu: "cpu"
        case .memory: "memorychip"
        case .temperature: "thermometer.medium"
        case .disk: "internaldrive"
        case .network: "arrow.up.arrow.down"
        }
    }

    var colors: [Color] {
        switch self {
        case .cpu: [.blue, Color(red: 0.1, green: 0.35, blue: 0.9)]
        case .memory: [.green, Color(red: 0.1, green: 0.55, blue: 0.25)]
        case .temperature: [.orange, Color(red: 0.9, green: 0.35, blue: 0.1)]
        case .disk: [Color(white: 0.55), Color(white: 0.3)]
        case .network: [.cyan, .blue]
        }
    }
}

struct Modules: RawRepresentable, Equatable {
    static let defaults = Modules([.cpu, .memory, .temperature, .disk])

    var enabled: [Module]

    init(_ enabled: [Module]) {
        self.enabled = enabled
    }

    init(rawValue: String) {
        enabled = rawValue.split(separator: ",").compactMap { Module(rawValue: String($0)) }
    }

    var rawValue: String {
        enabled.map(\.rawValue).joined(separator: ",")
    }
}

enum Health: Int, Comparable {
    case fine, warning, critical

    init(_ value: Double, warning: Double, critical: Double) {
        self = value >= critical ? .critical : value >= warning ? .warning : .fine
    }

    init(celsius: Double, critical: Double?) {
        let hot = (critical ?? 100) - 10
        self.init(celsius, warning: min(80, hot - 10), critical: hot)
    }

    func alert(_ words: [String]) -> String? {
        self == .fine ? nil : words[rawValue - 1]
    }

    static func < (lhs: Health, rhs: Health) -> Bool {
        lhs.rawValue < rhs.rawValue
    }

    var color: Color {
        switch self {
        case .fine: .green
        case .warning: .orange
        case .critical: .red
        }
    }
}

extension Space {
    var health: Health {
        Health(fraction, warning: 0.9, critical: 0.95)
    }
}

struct Point: Identifiable {
    let at: Date
    let value: Double

    var id: Date { at }
}

struct Measure {
    enum Visual {
        case spark([Point], ClosedRange<Double>, unit: String)
        case bar(Double)
        case steps(Int)
        case traffic(down: [Point], up: [Point])
    }

    let module: Module
    var name: String?
    let value: String
    var unit: String?
    let detail: String
    let headline: String
    var health = Health.fine
    var alerts: [String] = []
    let visual: Visual

    var title: String {
        name ?? module.title
    }

    var alert: String? {
        health.alert(alerts)
    }

    var spoken: String {
        [[value, unit].compactMap(\.self).joined(separator: " "), detail, alert].compactMap(\.self).joined(separator: ", ")
    }
}

extension Machine {
    func measures(_ modules: [Module]) -> [Measure] {
        modules.compactMap(measure)
    }

    func alert(_ modules: [Module]) -> Measure? {
        measures(modules).filter { $0.health > .fine }.max { $0.health < $1.health }
    }

    private func series(_ value: (Reading) -> Double?) -> [Point] {
        (history ?? []).compactMap { reading in value(reading).map { Point(at: Date(timeIntervalSince1970: reading.at), value: $0) } }
    }

    private func measure(_ module: Module) -> Measure? {
        switch module {
        case .cpu:
            let points = series { $0.cpu.map(Double.init) }
            guard let last = points.last?.value else { return nil }
            let sustained = min(last, points.dropLast().last?.value ?? last)
            return Measure(module: module, value: "\(Int(last))", unit: "%", detail: "Load \(tenths(load)) \u{00B7} \(cores) \(cores == 1 ? "core" : "cores")", headline: "\(Int(last))% CPU", health: Health(sustained, warning: 75, critical: 90), alerts: ["Busy", "Very busy"], visual: .spark(points, 0...100, unit: "percent"))
        case .memory:
            let (value, unit) = amount(memory)
            let notes = [pressure.map { "pressure \($0.word)" }, swap.map { $0.usedMB > 0 ? "\(amount(megabytes: $0.usedMB)) swap" : "no swap" }].compactMap(\.self).joined(separator: " \u{00B7} ")
            let health: Health = switch pressure {
            case .warning: .warning
            case .critical: .critical
            case .normal, nil: .fine
            }
            return Measure(module: module, value: value, unit: unit, detail: notes.prefix(1).uppercased() + notes.dropFirst(), headline: "\(Int((memory.fraction * 100).rounded()))% memory", health: health, alerts: ["Low memory", "Out of memory"], visual: .bar(memory.fraction))
        case .temperature:
            let points = series(\.temp)
            if let last = points.last?.value {
                let peak = points.map(\.value).max() ?? last
                return Measure(module: module, name: "CPU temp", value: "\(Int(last.rounded()))", unit: "\u{00B0}C", detail: "1h peak \(degrees(peak))", headline: degrees(last), health: Health(celsius: last, critical: critical), alerts: ["Warm", "Hot"], visual: .spark(points, 20...100, unit: "degrees Celsius"))
            }
            guard let thermal = series({ $0.thermal.map(Double.init) }).last.map({ min(Int($0.value), 3) }) else { return nil }
            let word = ["Normal", "Moderate", "Heavy", "Critical"][thermal]
            return Measure(module: module, name: "Thermal", value: word, detail: ["No throttling", "Throttling may start", "Throttling", "Throttling hard"][thermal], headline: "Thermal \(word.lowercased())", health: Health(Double(thermal), warning: 1, critical: 2), alerts: ["Warm", "Hot"], visual: .steps(thermal))
        case .disk:
            let (value, unit) = amount(disk)
            return Measure(module: module, value: value, unit: unit, detail: "\(amount(megabytes: disk.totalMB - disk.usedMB, rounded: true)) free", headline: "\(Int((disk.fraction * 100).rounded()))% disk", health: disk.health, alerts: ["Disk nearly full", "Disk full"], visual: .bar(disk.fraction))
        case .network:
            let down = series { $0.rx.map(Double.init) }
            let up = series { $0.tx.map(Double.init) }
            guard let received = down.last?.value, let sent = up.last?.value else { return nil }
            return Measure(module: module, value: "\u{2193} \(rate(received))", detail: "\u{2191} \(rate(sent))", headline: "\u{2193} \(rate(received))", visual: .traffic(down: down, up: up))
        }
    }
}

extension Pressure {
    var word: String {
        switch self {
        case .normal: "low"
        case .warning: "high"
        case .critical: "critical"
        }
    }
}

struct ScopeCaption: View {
    let machine: Machine
    let answers: Bool
    let now: Date
    @AppStorage(Defaults.modules) private var modules = Modules.defaults

    var body: some View {
        HStack(spacing: 0) {
            if answers, let alert = machine.alert(modules.enabled), let word = alert.alert {
                Text(word)
                    .fontWeight(.medium)
                    .foregroundStyle(alert.health.color)
                if !facts.isEmpty {
                    Text(" \u{00B7} ")
                }
            }
            Text(facts.joined(separator: " \u{00B7} "))
        }
    }

    private var facts: [String] {
        [
            answers ? nil : machine.quiet(now: now),
            machine.bootedAt.map { "up \(span(now.timeIntervalSince($0)))" },
            machine.battery.flatMap { $0.onBattery ? "on battery \($0.percent)%" : nil },
            machine.relayed == true ? "relayed" : nil,
        ].compactMap(\.self)
    }
}

struct Monitor: View {
    let name: String
    let machine: Machine
    let now: Date
    let open: ((Module) -> Void)?
    @AppStorage(Defaults.modules) private var modules = Modules.defaults

    private var rows: [[Measure]] {
        var rows: [[Measure]] = []
        var pending: [Measure] = []
        for measure in machine.measures(modules.enabled) {
            if measure.module == .network {
                if !pending.isEmpty { rows.append(pending) }
                rows.append([measure])
                pending = []
            } else {
                pending.append(measure)
                if pending.count == 2 {
                    rows.append(pending)
                    pending = []
                }
            }
        }
        return pending.isEmpty ? rows : rows + [pending]
    }

    var body: some View {
        Grid(horizontalSpacing: 8, verticalSpacing: 8) {
            ForEach(rows, id: \.first?.module) { row in
                GridRow {
                    ForEach(row, id: \.module) { measure in
                        Plate(name: name, measure: measure, now: now, open: machine.hasDetail(measure.module) ? open.map { open in { open(measure.module) } } : nil) {
                            switch measure.visual {
                            case let .spark(points, domain, _):
                                Card(measure: measure) {
                                    ZStack {
                                        Spark(points: points, domain: domain, color: measure.health.color, now: now)
                                            .id(name)
                                    }
                                }
                            case let .bar(fraction):
                                Card(measure: measure) {
                                    Level(fraction: fraction, tint: measure.health.color)
                                }
                            case let .steps(level):
                                Card(measure: measure) {
                                    Steps(level: level, tint: measure.health.color)
                                }
                            case let .traffic(down, up):
                                WideCard(name: name, measure: measure, down: down, up: up, now: now)
                            }
                        }
                        .gridCellColumns(measure.module == .network ? 2 : 1)
                    }
                }
            }
        }
    }
}

struct CardLabel: View {
    let measure: Measure

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: measure.module.symbol)
                .font(.system(size: 10, weight: .semibold))
            Text(measure.title)
                .font(.footnote.weight(.semibold))
        }
        .foregroundStyle(.secondary)
    }
}

struct Plate<Content: View>: View {
    let name: String
    let measure: Measure
    let now: Date
    let open: (() -> Void)?
    @ViewBuilder let content: Content
    @State private var hovered = false
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.accessibilityShowBorders) private var showBorders

    var body: some View {
        if let open {
            Button(action: open) {
                plated
            }
            .buttonStyle(Press())
            .help("Show \(measure.module.heading.lowercased())")
            .spoken(name: name, measure: measure, now: now)
            .accessibilityHint("Shows \(measure.module.heading.lowercased())")
        } else {
            plated
                .spoken(name: name, measure: measure, now: now)
        }
    }

    private var plated: some View {
        let shape = ConcentricRectangle(corners: .concentric(minimum: 8), isUniform: true)
        return content
            .background {
                shape
                    .fill(hovered && open != nil ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.quaternary))
                    .overlay {
                        if open != nil, contrast == .increased || showBorders {
                            shape.stroke(.separator)
                        }
                    }
            }
            .animation(.easeOut(duration: 0.15), value: hovered)
            .onHover { hovered = $0 }
    }
}

struct Card<Graphic: View>: View {
    let measure: Measure
    @ViewBuilder let graphic: Graphic

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            CardLabel(measure: measure)
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text(measure.value)
                    .font(.title2.weight(.semibold))
                    .monospacedDigit()
                    .contentTransition(.opacity)
                if let unit = measure.unit {
                    Text(unit)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.top, 3)
            .animation(.easeOut(duration: 0.2), value: measure.value)
            Spacer(minLength: 6)
            graphic
                .frame(height: 22, alignment: .bottom)
            Text(measure.detail)
                .font(.footnote)
                .monospacedDigit()
                .foregroundStyle(measure.health == .fine ? AnyShapeStyle(.secondary) : AnyShapeStyle(measure.health.color))
                .lineLimit(1)
                .contentTransition(.opacity)
                .animation(.easeOut(duration: 0.2), value: measure.detail)
                .padding(.top, 5)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: 96)
    }
}

struct WideCard: View {
    let name: String
    let measure: Measure
    let down: [Point]
    let up: [Point]
    let now: Date

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                CardLabel(measure: measure)
                Text(measure.value)
                    .font(.title3.weight(.semibold))
                    .monospacedDigit()
                    .contentTransition(.opacity)
                Text(measure.detail)
                    .font(.footnote)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                    .contentTransition(.opacity)
            }
            .fixedSize()
            .animation(.easeOut(duration: 0.2), value: measure.value)
            Spacer(minLength: 0)
            ZStack {
                Traffic(down: down, up: up, now: now)
                    .id(name)
            }
            .frame(width: 180, height: 40)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: 72)
    }
}

extension View {
    @ViewBuilder func spoken(name: String, measure: Measure, now: Date) -> some View {
        let element = accessibilityElement(children: .ignore)
            .accessibilityLabel(measure.title)
            .accessibilityValue(measure.spoken)
        if case let .spark(points, domain, unit) = measure.visual {
            element.accessibilityChartDescriptor(SparkDescriptor(title: "\(measure.title) on \(name), last hour", unit: unit, points: points, domain: domain, now: now))
        } else {
            element
        }
    }
}

struct Plot {
    let points: [Point]
    let start: Date
    let end: Date

    static let window: TimeInterval = 60 * 60

    init(_ points: [Point], now: Date) {
        end = max(now, points.last?.at ?? now)
        start = end.addingTimeInterval(-min(Plot.window, max(60, end.timeIntervalSince(points.first?.at ?? end))))
        self.points = points.count == 1 ? [start, end].map { Point(at: $0, value: points[0].value) } : points.unique
    }
}

struct Spark: View {
    let points: [Point]
    let domain: ClosedRange<Double>
    let color: Color
    let now: Date
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        let plot = Plot(points, now: now)
        let values = plot.points.map(\.value) + [domain.lowerBound]
        let height = { (value: Double) in UnitPoint(x: 0.5, y: (domain.upperBound - value) / (domain.upperBound - domain.lowerBound)) }
        ZStack {
            Trace(points: plot.points, start: plot.start, end: plot.end, domain: domain, part: .area(floor: domain.lowerBound))
                .fill(LinearGradient(colors: [color.opacity(0.28), color.opacity(0.04)], startPoint: height(values.max()!), endPoint: height(values.min()!)))
            Trace(points: plot.points, start: plot.start, end: plot.end, domain: domain, part: .line)
                .stroke(color, style: StrokeStyle(lineWidth: contrast == .increased ? 2 : 1.25, lineCap: .round, lineJoin: .round))
            Trace(points: plot.points, start: plot.start, end: plot.end, domain: domain, part: .end(area: 14))
                .fill(color)
        }
    }
}

struct Traffic: View {
    let down: [Point]
    let up: [Point]
    let now: Date

    var body: some View {
        let received = Plot(down, now: now)
        let sent = Plot(up, now: now)
        let top = max(1, (down + up).map(\.value).max() ?? 1)
        ZStack {
            Trace(points: received.points, start: received.start, end: received.end, domain: -top...top, part: .area(floor: 0))
                .fill(Color.blue.opacity(0.55))
            Trace(points: sent.points.map { Point(at: $0.at, value: -$0.value) }, start: received.start, end: received.end, domain: -top...top, part: .area(floor: 0))
                .fill(Color.purple.opacity(0.55))
            Trace(points: [received.start, received.end].map { Point(at: $0, value: 0) }, start: received.start, end: received.end, domain: -top...top, part: .line)
                .stroke(.separator, lineWidth: 0.5)
                .foregroundStyle(.blue)
        }
    }
}

struct Trace: Shape {
    enum Part {
        case line
        case area(floor: Double)
        case end(area: CGFloat)
    }

    let points: [Point]
    let start: Date
    let end: Date
    let domain: ClosedRange<Double>
    let part: Part

    func path(in rect: CGRect) -> Path {
        let height = { (value: Double) in rect.maxY - rect.height * (value - domain.lowerBound) / (domain.upperBound - domain.lowerBound) }
        let spots = points.map { CGPoint(x: rect.minX + rect.width * $0.at.timeIntervalSince(start) / end.timeIntervalSince(start), y: height($0.value)) }
        var path = Path()
        guard let first = spots.first, let last = spots.last else { return path }
        switch part {
        case let .end(area):
            let radius = (area / .pi).squareRoot()
            path.addEllipse(in: CGRect(x: last.x - radius, y: last.y - radius, width: radius * 2, height: radius * 2))
        case .line:
            path.addMonotoneCurve(through: spots)
        case let .area(floor):
            path.addMonotoneCurve(through: spots)
            path.addLine(to: CGPoint(x: last.x, y: height(floor)))
            path.addLine(to: CGPoint(x: first.x, y: height(floor)))
            path.closeSubpath()
        }
        return path
    }
}

extension Path {
    mutating func addMonotoneCurve(through spots: [CGPoint]) {
        move(to: spots[0])
        guard spots.count > 2 else {
            spots.dropFirst().forEach { addLine(to: $0) }
            return
        }
        let slopes = zip(spots, spots.dropFirst()).map { ($1.y - $0.y) / ($1.x - $0.x) }
        let widths = zip(spots, spots.dropFirst()).map { $1.x - $0.x }
        var tangents = spots.indices.dropFirst().dropLast().map { index in
            let (before, after) = (slopes[index - 1], slopes[index])
            let blend = (before * widths[index] + after * widths[index - 1]) / (widths[index - 1] + widths[index])
            return before * after > 0 ? CGFloat(signOf: before, magnitudeOf: min(2 * abs(before), 2 * abs(after), abs(blend))) : 0
        }
        tangents.insert((3 * slopes[0] - tangents[0]) / 2, at: 0)
        tangents.append((3 * slopes[slopes.count - 1] - tangents[tangents.count - 1]) / 2)
        for index in spots.indices.dropLast() {
            let (from, to) = (spots[index], spots[index + 1])
            let third = (to.x - from.x) / 3
            addCurve(to: to, control1: CGPoint(x: from.x + third, y: from.y + third * tangents[index]), control2: CGPoint(x: to.x - third, y: to.y - third * tangents[index + 1]))
        }
    }
}

struct Level: View {
    let fraction: Double
    let tint: Color
    var minimum: CGFloat = 6

    var body: some View {
        Capsule()
            .fill(.quaternary)
            .frame(height: 6)
            .overlay(alignment: .leading) {
                GeometryReader { proxy in
                    Capsule()
                        .fill(tint)
                        .frame(width: max(minimum, proxy.size.width * min(1, fraction)))
                }
            }
    }
}

struct Steps: View {
    let level: Int
    let tint: Color

    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<4, id: \.self) { step in
                Capsule()
                    .fill(step <= level ? AnyShapeStyle(tint) : AnyShapeStyle(.quaternary))
                    .frame(height: 6)
            }
        }
    }
}

struct SparkDescriptor: AXChartDescriptorRepresentable {
    let title: String
    let unit: String
    let points: [Point]
    let domain: ClosedRange<Double>
    let now: Date

    func makeChartDescriptor() -> AXChartDescriptor {
        let end = Plot(points, now: now).end
        let values = points.map(\.value)
        let minutes = AXNumericDataAxisDescriptor(title: "Minutes ago", range: -Plot.window / 60...0, gridlinePositions: []) { "\(Int(-$0)) minutes ago" }
        let value = AXNumericDataAxisDescriptor(title: unit, range: domain, gridlinePositions: []) { "\(Int($0.rounded())) \(unit)" }
        return AXChartDescriptor(
            title: title,
            summary: "Between \(Int(values.min() ?? 0)) and \(Int(values.max() ?? 0)) \(unit)",
            xAxis: minutes,
            yAxis: value,
            additionalAxes: [],
            series: [AXDataSeriesDescriptor(name: title, isContinuous: true, dataPoints: points.map { AXDataPoint(x: $0.at.timeIntervalSince(end) / 60, y: $0.value) })]
        )
    }
}

struct Usage: View {
    let session: Session
    let now: Date

    private var points: [Point] {
        (session.cpuHistory ?? []).map { Point(at: Date(timeIntervalSince1970: $0.at), value: Double($0.cpu)) }
    }

    var body: some View {
        HStack(spacing: 8) {
            Spark(points: points, domain: 0...max(100, points.map(\.value).max() ?? 0), color: Color(nsColor: .secondaryLabelColor), now: now)
                .frame(width: 44, height: 16)
            VStack(alignment: .trailing, spacing: 0) {
                Text("\(session.cpu)%")
                    .contentTransition(.opacity)
                    .animation(.easeOut(duration: 0.2), value: session.cpu)
                Text(session.memory)
                    .foregroundStyle(.secondary)
            }
            .font(.subheadline)
            .monospacedDigit()
            .frame(width: 56, alignment: .trailing)
        }
    }
}

extension Session {
    var memory: String {
        amount(megabytes: memoryMB)
    }
}

func tenths(_ value: Double) -> String {
    value.formatted(.number.precision(.fractionLength(1)))
}

func amount(_ space: Space) -> (value: String, unit: String) {
    let (size, unit) = space.totalMB < 1 << 20 ? (1024.0, "GB") : (Double(1 << 20), "TB")
    return (figure(Double(space.usedMB) / size), "of \(figure(Double(space.totalMB) / size)) \(unit)")
}

func figure(_ value: Double) -> String {
    value.formatted(.number.precision(.fractionLength(value < 100 ? 0...1 : 0...0)))
}

func amount(megabytes: Int, rounded: Bool = false) -> String {
    if megabytes < 1000 { return "\(megabytes) MB" }
    guard megabytes >= 1 << 20 else { return "\((Double(megabytes) / 1024).formatted(.number.precision(.fractionLength(rounded ? 0 : 1)))) GB" }
    return "\(tenths(Double(megabytes) / Double(1 << 20))) TB"
}

func rate(_ bytes: Double) -> String {
    let (value, unit) = bytes < 999_500 ? (bytes / 1e3, "kB") : bytes < 999_500_000 ? (bytes / 1e6, "MB") : (bytes / 1e9, "GB")
    return "\(figure(value)) \(unit)/s"
}

func degrees(_ celsius: Double) -> String {
    "\(Int(celsius.rounded())) \u{00B0}C"
}
