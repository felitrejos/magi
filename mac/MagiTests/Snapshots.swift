import AppKit
import SwiftUI
import Testing

@MainActor
@Suite(.enabled(if: ProcessInfo.processInfo.environment["SNAPSHOTS"] != nil), .serialized)
struct Snapshots {
    let out = URL(filePath: ProcessInfo.processInfo.environment["SNAPSHOTS"] ?? "")
    let now = Date.now
    let home = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)

    @Test func panel() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let full = try await store(machines(sessions: true))
        let quiet = try await store(machines(sessions: false))
        let bare = try await store([:])
        let strained = try await store(machines(sessions: true, critical: true))
        for scheme in [ColorScheme.light, .dark] {
            let suffix = scheme == .light ? "light" : "dark"
            try await shot("main-\(suffix)", full, scheme)
            try await shot("waiting-row-\(suffix)", full, scheme, act: { window in
                window.sendEvent(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, characters: "\u{F701}", charactersIgnoringModifiers: "\u{F701}", isARepeat: false, keyCode: 125)!)
            })
            try await shot("scope-\(suffix)", full, scheme, height: 900, act: { right($0, times: 2) })
            try await shot("scope-laptop-\(suffix)", full, scheme, height: 900, act: { right($0, times: 3) })
            try await shot("scope-alert-\(suffix)", strained, scheme, height: 900, act: { right($0, times: 3) })
            UserDefaults.standard.set("cpu,memory,temperature,disk,network", forKey: "monitorModules")
            try await shot("scope-network-\(suffix)", full, scheme, height: 900, act: { right($0, times: 2) })
            UserDefaults.standard.removeObject(forKey: "monitorModules")
            try await shot("empty-\(suffix)", quiet, scheme)
            try await shot("not-set-up-\(suffix)", bare, scheme)
            try await shot("health-\(suffix)", strained, scheme)
            try await shot("reduce-motion-\(suffix)", full, scheme, environment: { AnyView($0.environment(\._accessibilityReduceMotion, true)) })
        }
        try await shot("reduce-transparency", full, .light, opaque: true, environment: { AnyView($0.environment(\._accessibilityReduceTransparency, true)) })
        try await shot("increase-contrast", full, .light, environment: { AnyView($0.environment(\._colorSchemeContrast, .increased)) })
        try await shot("increase-contrast-dark", full, .dark, environment: { AnyView($0.environment(\._colorSchemeContrast, .increased)) })
        try await shot("show-borders", full, .light, environment: { AnyView($0.environment(\._accessibilityShowButtonShapes, true)) })
        try await shot("show-borders-scope", full, .light, environment: { AnyView($0.environment(\._accessibilityShowButtonShapes, true)) }, act: { right($0, times: 2) })
    }

    @Test func tiles() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let names = ["homelab", "laptop", "box"]
        let states = ["at rest", "selected"]
        for (kind, store) in [("normal", try await store(machines(sessions: true))), ("alert", try await store(machines(sessions: true, critical: true)))] {
            for scheme in [ColorScheme.light, .dark] {
                let grid = VStack(alignment: .leading, spacing: 6) {
                    ForEach(1...3, id: \.self) { count in
                        ForEach(states, id: \.self) { state in
                            Text("\(count) machine\(count == 1 ? "" : "s"), first \(state)")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                            HStack(spacing: 6) {
                                ForEach(names.prefix(count), id: \.self) { name in
                                    Tile(name: name, machine: store.machines[name]!, answers: true, selected: state == "selected" && name == names[0], now: now)
                                }
                            }
                            .frame(width: count == 1 ? 150 : 344)
                        }
                    }
                }
                .padding(12)
                .background(scheme == .light ? Color(white: 0.97) : Color(white: 0.14))
                .ignoresSafeArea()
                try await render("tiles-\(kind)-\(scheme == .light ? "light" : "dark")", grid, scheme)
            }
        }
    }

    @Test func settings() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let full = try await store(machines(sessions: true))
        let bare = try await store([:])
        let quiet = try await store(machines(sessions: false).mapValues { $0.merging(["answering": false]) { $1 } })
        UserDefaults.standard.set("cpu,memory,temperature,disk,network", forKey: "monitorModules")
        let scenes = NSHostingSceneRepresentation { Settings { SettingsView(store: full) } }
        NSApplication.shared.addSceneRepresentation(scenes)
        let before = Set(NSApp.windows)
        scenes.environment.openSettings()
        try await Task.sleep(for: .milliseconds(500))
        let window = try #require(NSApp.windows.first { !before.contains($0) })
        window.setFrameOrigin(CGPoint(x: -4000, y: -4000))
        for scheme in [ColorScheme.light, .dark] {
            let suffix = scheme == .light ? "light" : "dark"
            NSApp.appearance = NSAppearance(named: scheme == .light ? .aqua : .darkAqua)
            for pane in Pane.allCases {
                UserDefaults.standard.set(pane.rawValue, forKey: "pane")
                try await Task.sleep(for: .milliseconds(800))
                try capture(window, "settings-\(pane.rawValue)-\(suffix)")
            }
            for (name, store) in [("first-run", full), ("first-run-not-set-up", bare), ("first-run-quiet", quiet)] {
                try await render("\(name)-\(suffix)", Welcome(store: store).background(SettingsColor.content), scheme)
            }
        }
        window.close()
        NSApp.appearance = nil
        UserDefaults.standard.removeObject(forKey: "pane")
        UserDefaults.standard.removeObject(forKey: "monitorModules")
    }

    @Test func glyph() throws {
        let bundle = Bundle(for: Marker.self)
        let config = NSImage.SymbolConfiguration(textStyle: .body, scale: .large)
        let scale: CGFloat = 4
        let cell = CGSize(width: 44, height: 24)
        for dark in [false, true] {
            let size = CGSize(width: cell.width * 4, height: cell.height)
            let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale), pixelsHigh: Int(size.height * scale), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
            rep.size = size
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
            (dark ? NSColor(white: 0.17, alpha: 1) : NSColor(white: 0.93, alpha: 1)).setFill()
            NSRect(origin: .zero, size: size).fill()
            let ink = dark ? NSColor.white : NSColor.black
            let tinted = config.applying(.init(paletteColors: [ink]))
            let dotted = config.applying(.init(paletteColors: [ink, .systemOrange]))
            let symbols = [("orbit", tinted), ("orbit.badge", tinted), ("orbit.badge", dotted), ("orbit.slash", tinted)].map { NSImage(symbolName: $0.0, bundle: bundle, variableValue: 0)!.withSymbolConfiguration($0.1)! }
            for (column, image) in symbols.enumerated() {
                let x = CGFloat(column) * cell.width + (cell.width - image.size.width) / 2
                image.draw(in: CGRect(x: x, y: (cell.height - image.size.height) / 2, width: image.size.width, height: image.size.height))
            }
            NSGraphicsContext.restoreGraphicsState()
            try rep.representation(using: .png, properties: [:])!.write(to: out.appending(path: "glyph-\(dark ? "dark" : "light").png"))
        }
    }

    private func shot(_ name: String, _ store: Store, _ scheme: ColorScheme, height: CGFloat = 560, opaque: Bool = false, environment: (AnyView) -> AnyView = { $0 }, act: (NSWindow) -> Void = { _ in }) async throws {
        let fill = opaque ? Color(nsColor: .windowBackgroundColor) : (scheme == .light ? Color(white: 0.97, opacity: 0.94) : Color(white: 0.14, opacity: 0.94))
        let root = environment(AnyView(PanelView(store: store, height: height) {}))
            .background(fill, in: .rect(cornerRadius: PanelView.radius))
            .environment(\.colorScheme, scheme)
        let host = NSHostingView(rootView: root)
        let window = KeyWindow(contentRect: CGRect(origin: .zero, size: host.fittingSize), styleMask: .borderless, backing: .buffered, defer: false)
        window.level = NSWindow.Level(Int(CGWindowLevelForKey(.desktopWindow)) - 1)
        window.appearance = NSAppearance(named: scheme == .light ? .aqua : .darkAqua)
        window.backgroundColor = scheme == .light ? NSColor(white: 0.55, alpha: 1) : NSColor(white: 0.08, alpha: 1)
        window.contentView = host
        window.orderFrontRegardless()
        window.makeKey()
        try await Task.sleep(for: .milliseconds(300))
        act(window)
        try await Task.sleep(for: .milliseconds(900))
        window.setContentSize(CGSize(width: host.fittingSize.width, height: min(host.fittingSize.height, height)))
        try await Task.sleep(for: .milliseconds(300))
        try capture(window, name)
        window.orderOut(nil)
    }

    private func capture(_ window: NSWindow, _ name: String) throws {
        try Process.run(URL(filePath: "/usr/sbin/screencapture"), arguments: ["-o", "-x", "-l", "\(window.windowNumber)", out.appending(path: "\(name).png").path]).waitUntilExit()
    }

    private func render(_ name: String, _ view: some View, _ scheme: ColorScheme) async throws {
        let host = NSHostingView(rootView: view.environment(\.colorScheme, scheme))
        let window = KeyWindow(contentRect: CGRect(origin: CGPoint(x: -4000, y: -4000), size: host.fittingSize), styleMask: [.titled, .fullSizeContentView], backing: .buffered, defer: false)
        window.titlebarAppearsTransparent = true
        window.appearance = NSAppearance(named: scheme == .light ? .aqua : .darkAqua)
        window.contentView = host
        window.orderFrontRegardless()
        try await Task.sleep(for: .milliseconds(800))
        let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds)!
        host.cacheDisplay(in: host.bounds, to: rep)
        try rep.representation(using: .png, properties: [:])!.write(to: out.appending(path: "\(name).png"))
        window.orderOut(nil)
    }

    private func right(_ window: NSWindow, times: Int) {
        for _ in 0..<times {
            window.sendEvent(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, characters: "\u{F703}", charactersIgnoringModifiers: "\u{F703}", isARepeat: false, keyCode: 124)!)
        }
    }

    private func store(_ machines: [String: [String: Any]]) async throws -> Store {
        let home = home.appending(path: UUID().uuidString)
        let store = Store(home: home)
        for (index, (name, machine)) in machines.sorted(by: { $0.key < $1.key }).enumerated() {
            var machine = machine
            let load = machine["load"] as! Double
            let used = (machine["memory"] as! [String: Int])["usedMB"]!
            machine["heard"] = stamp(now.addingTimeInterval(-3))
            let sample = machine.removeValue(forKey: "sample") as! [String: Int]
            machine["history"] = (0..<120).map { step -> [String: Any] in
                let wave = sin(Double(step + index * 17) / 9) + 0.4 * sin(Double(step) / 2.3)
                let spike = max(0, 1 - abs(Double(step - 96)) / 6)
                var reading: [String: Any] = ["at": now.timeIntervalSince1970 - Double(119 - step) * 30, "load": max(0.05, load * (1 + 0.4 * wave)), "memoryMB": Int(Double(used) * (0.94 + 0.04 * wave))]
                reading["cpu"] = sample["cpu"].map { Int(min(100, max(1, step == 119 ? Double($0) : Double($0) * (1 + 0.25 * wave) + 30 * spike))) }
                reading["temp"] = sample["temp"].map { step == 119 ? Double($0) : Double($0) + 2 * wave + 14 * spike }
                reading["thermal"] = sample["thermal"]
                reading["rx"] = Int(max(50_000, 1_600_000 * (1 + 0.6 * wave) + 6_000_000 * spike))
                reading["tx"] = Int(max(20_000, 310_000 * (1 + 0.5 * wave)))
                return reading
            }
            machine["sessions"] = (machine["sessions"] as! [[String: Any]]).enumerated().map { number, session in
                var session = session
                let busy = session["state"] as? String != "idle"
                session["cpuHistory"] = (0..<120).map { step -> [String: Any] in
                    ["at": now.timeIntervalSince1970 - Double(119 - step) * 30, "cpu": busy ? Int(max(0, 40 + 38 * sin(Double(step + number * 7) / 4))) : step % 9 == 0 ? 3 : 0]
                }
                return session
            }
            try publish(JSONSerialization.data(withJSONObject: machine), as: name, in: home)
        }
        await until { store.machines.count == machines.count }
        return store
    }

    private func machines(sessions: Bool, critical: Bool = false) -> [String: [String: Any]] {
        func session(_ title: String, _ folder: String, _ state: String, _ minutes: Double, cpu: Int, memory: Int, waitingOn: String? = nil) -> [String: Any] {
            var session: [String: Any] = ["id": UUID().uuidString, "title": title, "folder": folder, "state": state, "since": stamp(now.addingTimeInterval(-minutes * 60)), "cpu": cpu, "memoryMB": memory]
            session["waitingOn"] = waitingOn
            return session
        }
        return [
            "laptop": [
                "answering": true, "ssh": "", "cores": 10, "load": 2.4, "pressure": critical ? "warning" : "normal",
                "memory": ["usedMB": 13100, "totalMB": 16384], "disk": ["usedMB": 282192, "totalMB": 471482], "swap": ["usedMB": 1536, "totalMB": 4096],
                "bootedAt": stamp(now.addingTimeInterval(-3 * 86_400)), "battery": ["percent": critical ? 14 : 64, "onBattery": true],
                "sample": ["cpu": critical ? 61 : 18, "thermal": critical ? 1 : 0],
                "sessions": sessions ? [
                    session("Shop checkout fix", "/Users/sam/code/shop", "waiting", 4, cpu: 0, memory: 229, waitingOn: "Allow Bash to run npm test in the shop folder?"),
                    session("magi README demo videos", "/Users/sam/code/magi", "busy", 12, cpu: 34, memory: 547),
                    session("Blog drafts", "/Users/sam/code/blog", "idle", 190, cpu: 0, memory: 212),
                ] : [],
            ],
            "homelab": [
                "answering": true, "ssh": "homelab", "cores": 12, "load": 3.1, "relayed": true,
                "memory": ["usedMB": 6450, "totalMB": 15384], "disk": ["usedMB": 1212000, "totalMB": 1875626], "swap": ["usedMB": 0, "totalMB": 2048],
                "bootedAt": stamp(now.addingTimeInterval(-6 * 60)),
                "sample": ["cpu": 26, "temp": critical ? 83 : 54],
                "sessions": sessions ? [
                    session("Nightly backup audit with a long title that runs out of room", "/home/sam/ops/backups/nightly", "waiting", 11, cpu: 0, memory: 310, waitingOn: "Which retention policy should I keep for the nightly snapshots, 30 days or 90 days?"),
                    session("shop-db migration", "/home/sam/code/shop", "busy", 20, cpu: 82, memory: 1100),
                    session("infra notes", "/home/sam/notes", "idle", 1_500, cpu: 0, memory: 180),
                ] : [],
            ],
            "box": [
                "answering": critical, "ssh": "box", "cores": 8, "load": critical ? 5.2 : 0.1, "pressure": critical ? "critical" : "normal",
                "memory": ["usedMB": 1698, "totalMB": 15384], "disk": ["usedMB": critical ? 931000 : 24380, "totalMB": 975626],
                "sample": ["cpu": critical ? 95 : 4],
                "sessions": sessions ? [session("scratch", "/home/sam/scratch", "idle", 130, cpu: 0, memory: 120)] : [],
            ],
        ].filter { sessions || $0.key != "box" }
    }

    private func stamp(_ date: Date) -> String {
        date.formatted(.iso8601)
    }
}

private final class Marker {}

private final class KeyWindow: NSWindow {
    override var canBecomeKey: Bool { true }
}
