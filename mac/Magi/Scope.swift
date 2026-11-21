import Charts
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
    let title: String
    let value: String
    var unit: String?
    let detail: String
    let headline: String
    var health = Health.fine
    var alerts: [String] = []
    let visual: Visual

    var alert: String? {
        health == .fine ? nil : alerts[health.rawValue - 1]
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
            return Measure(module: module, title: "CPU", value: "\(Int(last))", unit: "%", detail: "load \(tenths(load)) \u{00B7} \(cores) cores", headline: "\(Int(last))% CPU", health: Health(sustained, warning: 75, critical: 90), alerts: ["Busy", "Very busy"], visual: .spark(points, 0...100, unit: "percent"))
        case .memory:
            let (value, unit) = amount(memory)
            let notes = [pressure.map { "Pressure \($0.word)" }, swap.map { $0.usedMB > 0 ? "\(amount(megabytes: $0.usedMB)) swap" : "no swap" }].compactMap(\.self)
            let health: Health = switch pressure {
            case .warning: .warning
            case .critical: .critical
            case .normal, nil: .fine
            }
            return Measure(module: module, title: "Memory", value: value, unit: unit, detail: notes.joined(separator: " \u{00B7} "), headline: "\(value) GB", health: health, alerts: ["Low memory", "Out of memory"], visual: .bar(memory.fraction))
        case .temperature:
            let points = series(\.temp)
            if let last = points.last?.value {
                let peak = points.map(\.value).max() ?? last
                let health: Health = last >= (critical.map { $0 - 10 } ?? 90) ? .critical : last >= 80 ? .warning : .fine
                return Measure(module: module, title: "CPU temp", value: "\(Int(last.rounded()))", unit: "\u{00B0}C", detail: "1 h peak \(Int(peak.rounded())) \u{00B0}C", headline: "\(Int(last.rounded())) \u{00B0}C", health: health, alerts: ["Warm", "Hot"], visual: .spark(points, 20...100, unit: "degrees Celsius"))
            }
            guard let thermal = series({ $0.thermal.map(Double.init) }).last.map({ min(Int($0.value), 3) }) else { return nil }
            let word = ["Normal", "Moderate", "Heavy", "Critical"][thermal]
            return Measure(module: module, title: "Thermal", value: word, detail: ["No throttling", "Throttling may start", "Throttling", "Throttling hard"][thermal], headline: "Thermal \(word.lowercased())", health: Health(Double(thermal), warning: 1, critical: 2), alerts: ["Warm", "Hot"], visual: .steps(thermal))
        case .disk:
            let (value, unit) = amount(disk)
            return Measure(module: module, title: "Disk", value: value, unit: unit, detail: "\(amount(megabytes: disk.totalMB - disk.usedMB, rounded: true)) free", headline: "\(Int((disk.fraction * 100).rounded()))% disk", health: Health(disk.fraction, warning: 0.9, critical: 0.95), alerts: ["Disk nearly full", "Disk full"], visual: .bar(disk.fraction))
        case .network:
            let down = series { $0.rx.map(Double.init) }
            let up = series { $0.tx.map(Double.init) }
            guard let received = down.last?.value, let sent = up.last?.value else { return nil }
            return Measure(module: module, title: "Network", value: "\u{2193} \(rate(received))", detail: "\u{2191} \(rate(sent))", headline: "\u{2193} \(rate(received))", visual: .traffic(down: down, up: up))
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
    @AppStorage("monitorModules") private var modules = Modules.defaults

    var body: some View {
        HStack(spacing: 0) {
            if answers, let alert = machine.alert(modules.enabled), let word = alert.alert {
                Text(word)
                    .fontWeight(.medium)
                    .foregroundStyle(alert.health.color)
                if !facts.isEmpty {
                    Text(" \u{00B7} ")
                        .foregroundStyle(.secondary)
                }
            }
            Text(facts.joined(separator: " \u{00B7} "))
                .foregroundStyle(.secondary)
        }
        .font(.subheadline)
        .monospacedDigit()
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
    @AppStorage("monitorModules") private var modules = Modules.defaults

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
                        if measure.module == .network {
                            WideCard(name: name, measure: measure, now: now)
                                .gridCellColumns(2)
                        } else {
                            Card(name: name, measure: measure, now: now)
                        }
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

struct Card: View {
    let name: String
    let measure: Measure
    let now: Date

    private var tint: Color {
        measure.health.color
    }

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
            Group {
                switch measure.visual {
                case let .spark(points, domain, _):
                    ZStack {
                        Spark(points: points, domain: domain, color: tint, now: now)
                            .id(name)
                    }
                case let .bar(fraction):
                    Level(fraction: fraction, tint: tint)
                case let .steps(level):
                    Steps(level: level, tint: tint)
                case .traffic:
                    EmptyView()
                }
            }
            .frame(height: 22, alignment: .bottom)
            Text(measure.detail)
                .font(.footnote)
                .monospacedDigit()
                .foregroundStyle(measure.health == .fine ? AnyShapeStyle(.secondary) : AnyShapeStyle(tint))
                .lineLimit(1)
                .contentTransition(.opacity)
                .animation(.easeOut(duration: 0.2), value: measure.detail)
                .padding(.top, 5)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: 96)
        .background(ConcentricRectangle(corners: .concentric(minimum: 8), isUniform: true).fill(.quaternary))
        .spoken(name: name, measure: measure, now: now)
    }
}

struct WideCard: View {
    let name: String
    let measure: Measure
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
            if case let .traffic(down, up) = measure.visual {
                ZStack {
                    Traffic(down: down, up: up, now: now)
                        .id(name)
                }
                .frame(width: 180, height: 40)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .frame(height: 72)
        .background(ConcentricRectangle(corners: .concentric(minimum: 8), isUniform: true).fill(.quaternary))
        .spoken(name: name, measure: measure, now: now)
    }
}

extension View {
    @ViewBuilder func spoken(name: String, measure: Measure, now: Date) -> some View {
        let element = accessibilityElement(children: .ignore)
            .accessibilityLabel(measure.title)
            .accessibilityValue([[measure.value, measure.unit].compactMap(\.self).joined(separator: " "), measure.detail, measure.alert].compactMap(\.self).joined(separator: ", "))
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

    init(_ points: [Point], now: Date) {
        end = latest(points.last?.at.timeIntervalSince1970, now: now)
        start = end.addingTimeInterval(-window(from: points.first?.at.timeIntervalSince1970, to: end.timeIntervalSince1970))
        self.points = points.count == 1 ? [start, end].map { Point(at: $0, value: points[0].value) } : points
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
        Chart {
            ForEach(plot.points) { point in
                AreaMark(x: .value("Time", point.at), yStart: .value("Floor", domain.lowerBound), yEnd: .value("Value", point.value))
                    .foregroundStyle(LinearGradient(colors: [color.opacity(0.28), color.opacity(0.04)], startPoint: .top, endPoint: .bottom))
                    .interpolationMethod(.monotone)
                LineMark(x: .value("Time", point.at), y: .value("Value", point.value))
                    .foregroundStyle(color)
                    .lineStyle(StrokeStyle(lineWidth: contrast == .increased ? 2 : 1.25, lineCap: .round, lineJoin: .round))
                    .interpolationMethod(.monotone)
            }
            if let last = plot.points.last {
                PointMark(x: .value("Time", last.at), y: .value("Value", last.value))
                    .foregroundStyle(color)
                    .symbolSize(14)
            }
        }
        .chartXScale(domain: plot.start...plot.end)
        .chartYScale(domain: domain)
        .chartXAxis(.hidden)
        .chartYAxis(.hidden)
        .chartLegend(.hidden)
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
        Chart {
            ForEach(received.points) { point in
                AreaMark(x: .value("Time", point.at), yStart: .value("Zero", 0), yEnd: .value("Download", point.value), series: .value("Direction", "Download"))
                    .foregroundStyle(Color.blue.opacity(0.55))
                    .interpolationMethod(.monotone)
            }
            ForEach(sent.points) { point in
                AreaMark(x: .value("Time", point.at), yStart: .value("Zero", 0), yEnd: .value("Upload", -point.value), series: .value("Direction", "Upload"))
                    .foregroundStyle(Color.purple.opacity(0.55))
                    .interpolationMethod(.monotone)
            }
            RuleMark(y: .value("Zero", 0))
                .foregroundStyle(.separator)
                .lineStyle(StrokeStyle(lineWidth: 0.5))
        }
        .chartXScale(domain: received.start...received.end)
        .chartYScale(domain: -top...top)
        .chartXAxis(.hidden)
        .chartYAxis(.hidden)
        .chartLegend(.hidden)
    }
}

struct Level: View {
    let fraction: Double
    let tint: Color

    var body: some View {
        Capsule()
            .fill(.quaternary)
            .frame(height: 6)
            .overlay(alignment: .leading) {
                GeometryReader { proxy in
                    Capsule()
                        .fill(tint)
                        .frame(width: max(6, proxy.size.width * min(1, fraction)))
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
        let minutes = AXNumericDataAxisDescriptor(title: "Minutes ago", range: -historyWindow / 60...0, gridlinePositions: []) { "\(Int(-$0)) minutes ago" }
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
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var readings: [CPUReading] {
        session.cpuHistory ?? []
    }

    var body: some View {
        HStack(spacing: 8) {
            Trend(points: trend(readings.map { ($0.at, Double($0.cpu)) }, top: max(100, Double(readings.map(\.cpu).max() ?? 0)), now: now), color: Color(nsColor: .secondaryLabelColor), delay: 0.1)
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
        Int64(memoryMB << 20).formatted(.byteCount(style: .memory))
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
    guard megabytes >= 1 << 20 else { return "\((Double(megabytes) / 1024).formatted(.number.precision(.fractionLength(rounded ? 0 : 1)))) GB" }
    return "\(tenths(Double(megabytes) / Double(1 << 20))) TB"
}

func rate(_ bytes: Double) -> String {
    Int64(bytes).formatted(.byteCount(style: .decimal)) + "/s"
}
