import Charts
import SwiftUI

struct ScopeCaption: View {
    let machine: Machine
    let answers: Bool
    let now: Date

    var body: some View {
        Text(facts.joined(separator: " \u{00B7} "))
            .font(.subheadline)
            .monospacedDigit()
            .foregroundStyle(.secondary)
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

struct ScopeSummary: View {
    let name: String
    let machine: Machine
    let now: Date
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline) {
                    Text("Load")
                        .font(.subheadline.weight(.semibold))
                    Spacer()
                    Text("\(tenths(machine.load)) on \(machine.cores) cores")
                        .font(.footnote)
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .contentTransition(reduceMotion ? .opacity : .numericText(value: machine.load))
                        .animation(.snappy(duration: 0.3), value: machine.load)
                }
                LoadChart(name: name, machine: machine, now: now)
            }
            Capacity(label: "Memory", space: machine.memory, tint: machine.pressure?.tint ?? .secondary, points: trend((machine.history ?? []).map { ($0.at, Double($0.memoryMB)) }, top: Double(machine.memory.totalMB), now: now), note: memoryNote, alert: nil)
            Capacity(label: "Disk", space: machine.disk, tint: machine.disk.nearlyFull ? .orange : .secondary, points: nil, note: nil, alert: machine.disk.nearlyFull ? "nearly full" : nil)
        }
        .padding(8)
    }

    private var memoryNote: String? {
        let notes = [
            machine.pressure.map { "pressure \($0.rawValue)" },
            machine.swap.flatMap { $0.totalMB > 0 ? "swap \(gigabytes($0))" : nil },
        ].compactMap(\.self)
        return notes.isEmpty ? nil : notes.joined(separator: " \u{00B7} ")
    }
}

struct LoadChart: View {
    let name: String
    let machine: Machine
    let now: Date
    @State private var drawn = false
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var end: Date {
        latest(machine.history?.last?.at, now: now)
    }

    private var start: Date {
        end.addingTimeInterval(-window(from: machine.history?.first?.at, to: end.timeIntervalSince1970))
    }

    private var readings: [Reading] {
        let history = machine.history ?? []
        guard history.count < 2 else { return history }
        let load = history.first?.load ?? machine.load
        return [start, end].map { Reading(at: $0.timeIntervalSince1970, load: load, memoryMB: 0) }
    }

    private var marks: [Date] {
        end.timeIntervalSince(start) < 10 * 60 ? [start] : [start, start.addingTimeInterval(end.timeIntervalSince(start) / 2)]
    }

    var body: some View {
        Chart {
            ForEach(readings, id: \.at) { reading in
                AreaMark(x: .value("Time", Date(timeIntervalSince1970: reading.at)), y: .value("Load", reading.load))
                    .foregroundStyle(machine.health.opacity(0.18))
                    .interpolationMethod(.monotone)
                LineMark(x: .value("Time", Date(timeIntervalSince1970: reading.at)), y: .value("Load", reading.load))
                    .foregroundStyle(machine.health)
                    .lineStyle(StrokeStyle(lineWidth: contrast == .increased ? 2.5 : 1.5))
                    .interpolationMethod(.monotone)
            }
            RuleMark(y: .value("Cores", machine.cores))
                .foregroundStyle(Color(nsColor: .tertiaryLabelColor))
                .lineStyle(StrokeStyle(lineWidth: contrast == .increased ? 1.5 : 0.75, dash: [3, 3]))
                .annotation(position: .bottom, alignment: .trailing, spacing: 2) {
                    Text("\(machine.cores) cores")
                        .font(.footnote)
                        .foregroundStyle(.tertiary)
                }
        }
        .chartXScale(domain: start...end)
        .chartYScale(domain: 0...machine.peak)
        .chartXAxis {
            AxisMarks(values: marks) { value in
                AxisValueLabel(anchor: .topLeading) {
                    if let date = value.as(Date.self) {
                        Text("\(max(1, Int((end.timeIntervalSince(date) / 60).rounded()))) min ago")
                    }
                }
                .font(.caption2)
                .foregroundStyle(Color(nsColor: .tertiaryLabelColor))
            }
        }
        .chartYAxis(.hidden)
        .chartPlotStyle { plot in
            plot.mask(alignment: .leading) {
                Rectangle().scaleEffect(x: drawn || reduceMotion ? 1 : 0, anchor: .leading)
            }
        }
        .frame(height: 84)
        .onAppear {
            withAnimation(.smooth(duration: 0.5)) {
                drawn = true
            }
        }
        .accessibilityChartDescriptor(LoadDescriptor(name: name, readings: machine.history ?? [], cores: machine.cores, top: machine.peak, end: end))
    }
}

struct LoadDescriptor: AXChartDescriptorRepresentable {
    let name: String
    let readings: [Reading]
    let cores: Int
    let top: Double
    let end: Date

    func makeChartDescriptor() -> AXChartDescriptor {
        let minutes = AXNumericDataAxisDescriptor(title: "Minutes ago", range: -historyWindow / 60...0, gridlinePositions: []) { "\(Int(-$0)) minutes ago" }
        let load = AXNumericDataAxisDescriptor(title: "Load", range: 0...top, gridlinePositions: [Double(cores)]) { "load \(tenths($0))" }
        let points = readings.map { AXDataPoint(x: ($0.at - end.timeIntervalSince1970) / 60, y: $0.load) }
        return AXChartDescriptor(
            title: "Load on \(name), last hour",
            summary: "Load between \(tenths(readings.map(\.load).min() ?? 0)) and \(tenths(readings.map(\.load).max() ?? 0)) on \(cores) cores",
            xAxis: minutes,
            yAxis: load,
            additionalAxes: [],
            series: [AXDataSeriesDescriptor(name: "Load", isContinuous: true, dataPoints: points)]
        )
    }
}

struct Capacity: View {
    let label: String
    let space: Space
    let tint: Color
    let points: [CGPoint]?
    let note: String?
    let alert: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Text(label)
                    .font(.subheadline.weight(.semibold))
                if let points {
                    Trend(points: points, color: tint, delay: 0.1)
                        .frame(width: 44, height: 12)
                }
                Spacer()
                Text(gigabytes(space))
                    .font(.footnote)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            Capsule()
                .fill(.quaternary)
                .frame(height: 5)
                .overlay(alignment: .leading) {
                    GeometryReader { proxy in
                        Capsule()
                            .fill(tint)
                            .frame(width: proxy.size.width * space.fraction)
                    }
                }
            if let note {
                Text(note)
                    .font(.footnote)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel([label, gigabytes(space), note, alert].compactMap(\.self).joined(separator: ", "))
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
                    .contentTransition(reduceMotion ? .opacity : .numericText(value: Double(session.cpu)))
                    .animation(.snappy(duration: 0.3), value: session.cpu)
                Text(session.memory)
                    .foregroundStyle(.secondary)
            }
            .font(.subheadline)
            .monospacedDigit()
            .frame(width: 56, alignment: .trailing)
        }
    }
}

extension Pressure {
    var tint: Color {
        switch self {
        case .normal: .secondary
        case .warning: .orange
        case .critical: .red
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

func gigabytes(_ space: Space) -> String {
    let (size, unit) = space.totalMB < 1 << 20 ? (1024.0, "GB") : (Double(1 << 20), "TB")
    return "\((Double(space.usedMB) / size).formatted(.number.precision(.fractionLength(1)))) of \((Double(space.totalMB) / size).formatted(.number.precision(.fractionLength(0...1)))) \(unit)"
}
