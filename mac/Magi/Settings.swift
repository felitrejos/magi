import ServiceManagement
import SwiftUI

private let repository = URL(string: "https://github.com/felitrejos/magi")!

enum Glyph: String, CaseIterable {
    case trio, orbit, bridge

    var resource: ImageResource {
        switch self {
        case .trio: .trio
        case .orbit: .orbit
        case .bridge: .bridge
        }
    }
}

enum Pane: String, CaseIterable {
    case general, menuBar, machines, sound, about

    static let groups: [[Pane]] = [[.general, .menuBar], [.machines, .sound], [.about]]

    var title: String {
        switch self {
        case .general: "General"
        case .menuBar: "Menu Bar"
        case .machines: "Machines"
        case .sound: "Sound"
        case .about: "About"
        }
    }

    var symbol: String {
        switch self {
        case .general: "gearshape"
        case .menuBar: "menubar.rectangle"
        case .machines: "server.rack"
        case .sound: "speaker.wave.2"
        case .about: "info.circle"
        }
    }

    var rows: [String] {
        switch self {
        case .general: ["Open magi at login", "Show the waiting count", "Colour the badge", "Setup steps"]
        case .menuBar: ["Icon"]
        case .machines: []
        case .sound: ["Play a sound", "Sound", "Volume"]
        case .about: ["Version", "GitHub Repository", "Report an Issue"]
        }
    }

    func matches(_ query: String) -> Bool {
        query.isEmpty || ([title] + rows).contains { $0.localizedStandardContains(query) }
    }
}

extension EnvironmentValues {
    @Entry var query = ""
}

struct SettingsView: View {
    let store: Store
    @AppStorage("pane") private var pane = Pane.general
    @State private var back: [Pane] = []
    @State private var forward: [Pane] = []
    @State private var query = ""

    var body: some View {
        NavigationSplitView {
            List(selection: Binding(get: { pane }, set: { $0.map(go) })) {
                Identity()
                    .selectionDisabled()
                ForEach(Pane.groups, id: \.self) { group in
                    Section {
                        ForEach(group.filter { $0.matches(query) }, id: \.self) { pane in
                            Label {
                                Text(pane.title)
                            } icon: {
                                Icon(symbol: pane.symbol, size: 24)
                            }
                            .tag(pane)
                        }
                    }
                }
            }
            .searchable(text: $query, placement: .sidebar, prompt: "Search settings\u{2026}")
            .navigationSplitViewColumnWidth(230)
            .toolbar(removing: .sidebarToggle)
        } detail: {
            Form {
                switch pane {
                case .general: General()
                case .menuBar: MenuBar()
                case .machines: Machines(store: store)
                case .sound: Sound()
                case .about: About()
                }
            }
            .formStyle(.grouped)
            .environment(\.query, query)
            .toolbar {
                ToolbarItemGroup(placement: .navigation) {
                    Button("Back", systemImage: "chevron.left") {
                        forward.append(pane)
                        pane = back.removeLast()
                    }
                    .disabled(back.isEmpty)
                    Button("Forward", systemImage: "chevron.right") {
                        back.append(pane)
                        pane = forward.removeLast()
                    }
                    .disabled(forward.isEmpty)
                }
            }
        }
        .toolbar(removing: .title)
        .tint(.accentColor)
        .frame(width: 680, height: 600)
    }

    private func go(_ next: Pane) {
        guard next != pane else { return }
        back.append(pane)
        forward = []
        pane = next
    }
}

struct Identity: View {
    var body: some View {
        HStack(spacing: 10) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 32, height: 32)
            VStack(alignment: .leading, spacing: 0) {
                Text("magi")
                    .font(.body.weight(.semibold))
                Text(version)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 6)
    }
}

struct Icon: View {
    let symbol: String
    let size: CGFloat

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: size * 0.5, weight: .medium))
            .foregroundStyle(.primary)
            .frame(width: size, height: size)
            .background(.quaternary, in: .rect(cornerRadius: size * 0.3, style: .continuous))
    }
}

struct Setting<Control: View>: View {
    let title: String
    var detail: String?
    @ViewBuilder let control: Control
    @Environment(\.query) private var query

    var body: some View {
        if query.isEmpty || title.localizedStandardContains(query) {
            LabeledContent {
                control
            } label: {
                Text(title)
                if let detail {
                    Text(detail)
                }
            }
            .frame(minHeight: 34)
        }
    }
}

struct General: View {
    @AppStorage("glyph") private var glyph = Glyph.trio
    @AppStorage("showsCount") private var showsCount = false
    @AppStorage("dot") private var dot = false
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openURL) private var openURL

    var body: some View {
        Section {
            HStack(spacing: 12) {
                Image(glyph.resource)
                    .font(.system(size: 22))
                    .foregroundStyle(.white)
                    .frame(width: 56, height: 40)
                    .background(.black.opacity(0.85), in: .rect(cornerRadius: 9, style: .continuous))
                VStack(alignment: .leading, spacing: 2) {
                    Text("Get started with magi")
                        .font(.body.weight(.semibold))
                    Text("Move sessions between your machines and see what waits on you.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                Button("Open Tutorial") {
                    openURL(repository.appending(path: "blob/main/docs/tutorial.md"))
                }
                .buttonStyle(.bordered)
            }
            .padding(.vertical, 6)
        }
        Section("Startup") {
            LoginItem()
        }
        Section("Menu bar") {
            Setting(title: "Show the waiting count", detail: "The number of sessions waiting on you, next to the icon.") {
                Toggle("", isOn: $showsCount).labelsHidden().toggleStyle(.switch)
            }
            Setting(title: "Colour the badge", detail: "A coloured dot instead of a cutout, so it stands out.") {
                Toggle("", isOn: $dot).labelsHidden().toggleStyle(.switch)
            }
        }
        Section("First run") {
            Setting(title: "Setup steps", detail: "Check that magi is set up here and choose whether it opens at login.") {
                Button("Show Again") {
                    NSApp.activate()
                    openWindow(id: "welcome")
                }
            }
        }
    }
}

struct MenuBar: View {
    @AppStorage("glyph") private var glyph = Glyph.trio
    @Environment(\.query) private var query

    var body: some View {
        if query.isEmpty || "Icon".localizedStandardContains(query) {
            Section("Appearance") {
                HStack(alignment: .top, spacing: 16) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Icon")
                        Text("How magi looks in the menu bar.")
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                    ForEach(Glyph.allCases, id: \.self) { candidate in
                        Button {
                            glyph = candidate
                        } label: {
                            VStack(spacing: 8) {
                                Image(candidate.resource)
                                    .font(.system(size: 16))
                                    .frame(width: 72, height: 30)
                                    .background(.quaternary, in: .rect(cornerRadius: 8, style: .continuous))
                                    .overlay {
                                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                                            .strokeBorder(glyph == candidate ? AnyShapeStyle(.tint) : AnyShapeStyle(.clear), lineWidth: 2)
                                    }
                                Text(candidate.rawValue.capitalized)
                                    .font(.subheadline.weight(glyph == candidate ? .semibold : .regular))
                                    .foregroundStyle(glyph == candidate ? AnyShapeStyle(.white) : AnyShapeStyle(.secondary))
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 2)
                                    .background(glyph == candidate ? AnyShapeStyle(.tint) : AnyShapeStyle(.clear), in: .capsule)
                            }
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("\(candidate.rawValue.capitalized) icon")
                        .accessibilityAddTraits(glyph == candidate ? .isSelected : [])
                    }
                }
                .padding(.vertical, 6)
            }
        }
    }
}

struct Machines: View {
    let store: Store

    var body: some View {
        Section {
            if store.machines.isEmpty {
                Text("No machines yet. Ask Claude to \u{201C}set up magi with my homelab\u{201D}.")
                    .foregroundStyle(.secondary)
            }
            ForEach(store.machines.keys.sorted(), id: \.self) { name in
                if let machine = store.machines[name] {
                    HStack(spacing: 10) {
                        Icon(symbol: machine.device, size: 28)
                        Setting(title: name, detail: machine.ssh?.isEmpty ?? true ? "This Mac" : "ssh \(machine.ssh ?? "")") {
                            Label(store.answering.contains(name) ? "Answering" : "Not answering", systemImage: "circle.fill")
                                .labelStyle(Dot(color: store.answering.contains(name) ? .green : .secondary))
                        }
                    }
                }
            }
        } header: {
            Text("Machines")
        } footer: {
            Text("magi adds a machine when you ask Claude to set it up.")
                .foregroundStyle(.secondary)
        }
    }
}

struct Dot: LabelStyle {
    let color: Color

    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 6) {
            configuration.icon
                .font(.system(size: 7))
                .foregroundStyle(color)
            configuration.title
                .foregroundStyle(.secondary)
        }
    }
}

struct Sound: View {
    @AppStorage("chimes") private var chimes = false
    @AppStorage("chime") private var chime = "Glass"
    @AppStorage("volume") private var volume = 0.5

    private var sounds: [String] {
        ((try? FileManager.default.contentsOfDirectory(atPath: "/System/Library/Sounds")) ?? []).map { ($0 as NSString).deletingPathExtension }.sorted()
    }

    var body: some View {
        Section("When a session starts waiting") {
            Setting(title: "Play a sound", detail: "Once, each time a session starts waiting on you.") {
                Toggle("", isOn: $chimes).labelsHidden().toggleStyle(.switch)
            }
            Setting(title: "Sound") {
                Picker("", selection: $chime) {
                    ForEach(sounds, id: \.self) { sound in
                        Label(sound, systemImage: "speaker.wave.2").tag(sound)
                    }
                }
                .labelsHidden()
                .fixedSize()
            }
            .disabled(!chimes)
            Setting(title: "Volume") {
                HStack(spacing: 8) {
                    Slider(value: $volume, in: 0...1)
                        .frame(width: 150)
                    Text(volume.formatted(.percent.precision(.fractionLength(0))))
                        .font(.callout)
                        .monospacedDigit()
                        .frame(minWidth: 40)
                        .padding(.vertical, 2)
                        .background(.quaternary, in: .capsule)
                    Button("Reset", systemImage: "arrow.counterclockwise") {
                        volume = 0.5
                    }
                    .labelStyle(.iconOnly)
                    .buttonBorderShape(.circle)
                    .disabled(volume == 0.5)
                }
            }
            .disabled(!chimes)
        }
    }
}

struct LoginItem: View {
    @State private var status = SMAppService.mainApp.status

    var body: some View {
        Setting(title: "Open magi at login", detail: status == .requiresApproval ? "macOS needs your approval in Login Items." : "magi stays in the menu bar and wakes only when a machine reports.") {
            if status == .requiresApproval {
                Button("Open Login Items") {
                    SMAppService.openSystemSettingsLoginItems()
                }
            } else {
                Toggle("", isOn: Binding(get: { status == .enabled }, set: { on in
                    try? on ? SMAppService.mainApp.register() : SMAppService.mainApp.unregister()
                    status = SMAppService.mainApp.status
                }))
                .labelsHidden()
                .toggleStyle(.switch)
            }
        }
    }
}

struct About: View {
    @Environment(\.openURL) private var openURL

    var body: some View {
        Section {
            Setting(title: "magi", detail: "Updates come through magi.") {
                Text(version)
            }
            Text("Claude is a trademark of Anthropic. magi is not affiliated with Anthropic.")
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
        Section("Feedback") {
            HStack(spacing: 12) {
                Button {
                    openURL(repository)
                } label: {
                    Text("GitHub Repository").frame(maxWidth: .infinity)
                }
                Button {
                    openURL(repository.appending(path: "issues/new"))
                } label: {
                    Text("Report an Issue").frame(maxWidth: .infinity)
                }
            }
            .controlSize(.large)
        }
    }
}

struct Welcome: View {
    let store: Store
    @State private var step = 0
    @AppStorage("welcomed") private var welcomed = false
    @Environment(\.dismissWindow) private var dismissWindow

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    HStack(spacing: 12) {
                        Icon(symbol: step == 0 ? "checkmark.seal" : "power", size: 36)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(step == 0 ? "Is magi set up here?" : "Open magi at login?")
                                .font(.title3.weight(.semibold))
                            Text("Step \(step + 1) of 2. You can skip it and run it again from Settings.")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                        }
                    }
                    .padding(.vertical, 6)
                }
                Section {
                    if step == 0 {
                        Setting(title: "Machines answering", detail: store.answering.isEmpty ? "Ask Claude to \u{201C}set up magi with my homelab\u{201D}, and this step checks itself." : "magi is set up here.") {
                            Text("\(store.answering.count) of \(store.machines.count)")
                                .monospacedDigit()
                        }
                    } else {
                        LoginItem()
                    }
                }
            }
            .formStyle(.grouped)
            HStack {
                Spacer()
                Button(step == 1 ? "Done" : store.answering.isEmpty ? "Skip" : "Continue") {
                    if step == 0 {
                        step = 1
                    } else {
                        welcomed = true
                        dismissWindow(id: "welcome")
                    }
                }
                .keyboardShortcut(.defaultAction)
            }
            .padding([.horizontal, .bottom], 20)
        }
        .frame(width: 480, height: 320)
    }
}

private var version: String {
    "Version \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "")"
}
