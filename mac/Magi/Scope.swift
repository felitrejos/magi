import Charts
import SwiftUI

private let window: TimeInterval = 30 * 60

struct ScopeCaption: View {
    let machine: Machine

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { context in
            Text([machine.heard.map { "heard \(Duration.seconds(max(0, context.date.timeIntervalSince($0).rounded())).formatted(.units(allowed: [.hours, .minutes, .seconds], width: .narrow, maximumUnitCount: 1))) ago" }, machine.bootedAt.map { "up \(span(context.date.timeIntervalSince($0)))" }].compactMap(\.self).joined(separator: ", "))
                .font(.subheadline)
                .monospacedDigit()
                .foregroundStyle(.secondary)
        }
    }
}

struct ScopeSummary: View {
    let name: String
    let machine: Machine
    let samples: [Sample]
    let now: Date
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("\(machine.load.formatted(.number.precision(.fractionLength(1)))) load, \(machine.cores) cores")
                .font(.subheadline.weight(.semibold))
                .monospacedDigit()
                .contentTransition(reduceMotion ? .opacity : .numericText(value: machine.load))
                .animation(.snappy(duration: 0.35), value: machine.load)
            if samples.count >= 3 {
                LoadChart(name: name, samples: samples, cores: machine.cores, now: now)
            }
            Capacity(label: "Memory", space: machine.memory, pressure: machine.pressure ?? .normal)
            Capacity(label: "Disk", space: machine.disk, pressure: machine.disk.pressure)
            if let swap = machine.swap {
                Text("Swap \(gigabytes(swap))")
                    .font(.footnote)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
        }
        .padding(8)
    }
}

struct LoadChart: View {
    let name: String
    let samples: [Sample]
    let cores: Int
    let now: Date
    @Environment(\.colorSchemeContrast) private var contrast

    private var top: Double {
        max(Double(cores), samples.map(\.value).max() ?? 0)
    }

    var body: some View {
        Chart {
            ForEach(samples, id: \.time) { sample in
                AreaMark(x: .value("Time", sample.time), y: .value("Load", sample.value))
                    .foregroundStyle(.tint.opacity(0.18))
                    .interpolationMethod(.monotone)
                LineMark(x: .value("Time", sample.time), y: .value("Load", sample.value))
                    .foregroundStyle(.tint)
                    .lineStyle(StrokeStyle(lineWidth: contrast == .increased ? 2.5 : 1.5))
                    .interpolationMethod(.monotone)
            }
            RuleMark(y: .value("Cores", cores))
                .foregroundStyle(.tertiary)
                .lineStyle(StrokeStyle(lineWidth: contrast == .increased ? 1.5 : 0.75, dash: [3, 3]))
                .annotation(position: .top, alignment: .trailing, spacing: 2) {
                    Text("\(cores) cores")
                        .font(.footnote)
                        .foregroundStyle(.tertiary)
                }
        }
        .chartXScale(domain: now.addingTimeInterval(-window)...now)
        .chartYScale(domain: 0...top)
        .chartXAxis(.hidden)
        .chartYAxis(.hidden)
        .frame(height: 72)
        .accessibilityChartDescriptor(LoadDescriptor(name: name, samples: samples, cores: cores, top: top, now: now))
    }
}

struct LoadDescriptor: AXChartDescriptorRepresentable {
    let name: String
    let samples: [Sample]
    let cores: Int
    let top: Double
    let now: Date

    func makeChartDescriptor() -> AXChartDescriptor {
        let minutes = AXNumericDataAxisDescriptor(title: "Minutes ago", range: -window / 60...0, gridlinePositions: []) { "\(Int(-$0)) minutes ago" }
        let load = AXNumericDataAxisDescriptor(title: "Load", range: 0...top, gridlinePositions: [Double(cores)]) { "load \($0.formatted(.number.precision(.fractionLength(1))))" }
        let points = samples.map { AXDataPoint(x: $0.time.timeIntervalSince(now) / 60, y: $0.value) }
        return AXChartDescriptor(
            title: "Load on \(name), last 30 minutes",
            summary: "Between \(samples.map(\.value).min()?.formatted() ?? "0") and \(samples.map(\.value).max()?.formatted() ?? "0"), on \(cores) cores",
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
    let pressure: Pressure

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text(label)
                Spacer()
                Text(gigabytes(space))
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            .font(.footnote)
            Capsule()
                .fill(.quaternary)
                .frame(height: 5)
                .overlay(alignment: .leading) {
                    GeometryReader { proxy in
                        Capsule()
                            .fill(pressure.tint)
                            .frame(width: proxy.size.width * min(space.fraction, 1))
                    }
                }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(label), \(gigabytes(space)), pressure \(pressure.rawValue)")
    }
}

struct History {
    let cpu: [Sample]
    let states: [Transition]
    let now: Date

    var waits: [ClosedRange<Date>] {
        zip(states, states.dropFirst().map(\.time) + [now]).filter { $0.0.state == "waiting" }.map { max($0.0.time, now.addingTimeInterval(-window))...max($0.1, $0.0.time) }
    }
}

struct Usage: View {
    let session: Session
    let history: History
    @Environment(\.colorSchemeContrast) private var contrast

    var body: some View {
        HStack(spacing: 8) {
            Chart {
                ForEach(history.waits, id: \.lowerBound) { wait in
                    RectangleMark(xStart: .value("Start", wait.lowerBound), xEnd: .value("End", wait.upperBound))
                        .foregroundStyle(.tint.opacity(0.2))
                }
                ForEach(history.cpu, id: \.time) { sample in
                    LineMark(x: .value("Time", sample.time), y: .value("CPU", sample.value))
                        .lineStyle(StrokeStyle(lineWidth: contrast == .increased ? 2 : 1.25))
                        .foregroundStyle(.secondary)
                }
            }
            .chartXScale(domain: history.now.addingTimeInterval(-window)...history.now)
            .chartYScale(domain: 0...max(100, history.cpu.map(\.value).max() ?? 0))
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .chartLegend(.hidden)
            .frame(width: 44, height: 16)
            .accessibilityLabel("CPU \(session.cpu)%, peak \(Int(history.cpu.map(\.value).max() ?? 0))% in the last 30 minutes")
            VStack(alignment: .trailing, spacing: 0) {
                Text("\(session.cpu)%")
                Text(Int64(session.memoryMB << 20).formatted(.byteCount(style: .memory)))
                    .foregroundStyle(.secondary)
            }
            .font(.subheadline)
            .monospacedDigit()
            .frame(width: 56, alignment: .trailing)
        }
    }
}

struct Moves: View {
    let moves: [Move]

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(moves, id: \.time) { move in
                if let note = move.note {
                    Text(note).lineLimit(2)
                } else {
                    Text("\(URL(filePath: move.toDir).lastPathComponent) to \(move.to), \(move.time.formatted(.relative(presentation: .named)))\(move.speed.map { ", \($0)" } ?? "")")
                }
            }
        }
        .font(.footnote)
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .leading)
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

func gigabytes(_ space: Space) -> String {
    let (size, unit) = space.totalMB < 1 << 20 ? (1024.0, "GB") : (Double(1 << 20), "TB")
    return "\((Double(space.usedMB) / size).formatted(.number.precision(.fractionLength(1)))) of \((Double(space.totalMB) / size).formatted(.number.precision(.fractionLength(0...1)))) \(unit)"
}
