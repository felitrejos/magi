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
            for (times, host) in ["rex", "claude", "ghostty", "tmux"].enumerated() {
                try await shot("open-\(host)-row-\(suffix)", full, scheme, act: { down($0, times: times + 1) })
            }
            try await shot("scope-\(suffix)", full, scheme, height: 900, act: { right($0, times: 2) })
            try await shot("scope-laptop-\(suffix)", full, scheme, height: 900, act: { right($0, times: 3) })
            try await shot("scope-alert-\(suffix)", strained, scheme, height: 900, act: { right($0, times: 3) })
            UserDefaults.standard.set("cpu,memory,temperature,disk,network", forKey: Defaults.modules)
            try await shot("scope-network-\(suffix)", full, scheme, height: 900, act: { right($0, times: 2) })
            try await shot("scope-increase-contrast-\(suffix)", full, scheme, height: 900, environment: { AnyView($0.environment(\._colorSchemeContrast, .increased)) }, act: { right($0, times: 2) })
            try await shot("scope-reduce-motion-\(suffix)", full, scheme, height: 900, environment: { AnyView($0.environment(\._accessibilityReduceMotion, true)) }, act: { right($0, times: 2) })
            UserDefaults.standard.removeObject(forKey: Defaults.modules)
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

    @Test func details() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        UserDefaults.standard.set("cpu,memory,temperature,disk,network", forKey: Defaults.modules)
        defer { UserDefaults.standard.removeObject(forKey: Defaults.modules) }
        let full = { try await store(machines(sessions: true)) }
        let strained = { try await store(machines(sessions: true, critical: true)) }
        var long = machines(sessions: true)
        long["Sams-MacBook-Pro-in-the-upstairs-office"] = long.removeValue(forKey: "laptop")
        for scheme in [ColorScheme.light, .dark] {
            let suffix = scheme == .light ? "light" : "dark"
            for (module, machine) in [(Module.cpu, 2), (.memory, 3), (.disk, 3), (.network, 2), (.temperature, 2)] {
                try await shot("detail-\(module.rawValue)-\(suffix)", full(), scheme, act: { try await open(module, on: machine, in: $0) })
            }
            try await shot("detail-cpu-highlight-\(suffix)", full(), scheme, act: { window in
                try await open(.cpu, on: 2, in: window)
                try await Task.sleep(for: .milliseconds(700))
                down(window, times: 1)
            })
            try await shot("detail-back-\(suffix)", full(), scheme, act: { window in
                let monitor = try await open(.memory, on: 3, in: window)
                let detail = window.contentView!.fittingSize
                try await tap(window, at: CGPoint(x: 120, y: 16))
                try await Task.sleep(for: .milliseconds(700))
                #expect(window.contentView!.fittingSize == detail, "module name")
                try await tap(window, at: CGPoint(x: 30, y: 16))
                try await Task.sleep(for: .milliseconds(700))
                #expect(window.contentView!.fittingSize == monitor, "machine name")
                try await open(.memory, on: 0, in: window)
                press(window, "\u{1B}", keyCode: 53, times: 1)
                try await Task.sleep(for: .milliseconds(700))
                #expect(window.contentView!.fittingSize == monitor, "escape")
            })
            try await shot("detail-single-memory-\(suffix)", store(machines(sessions: true).filter { $0.key == "laptop" }), scheme, act: { try await open(.memory, on: 1, in: $0) })
            try await shot("detail-long-name-\(suffix)", store(long), scheme, act: { try await open(.memory, on: 1, in: $0) })
            try await shot("detail-single-long-name-\(suffix)", store(long.filter { $0.key.hasPrefix("Sams") }), scheme, act: { try await open(.memory, on: 1, in: $0) })
            try await shot("detail-alert-disk-\(suffix)", strained(), scheme, act: { try await open(.disk, on: 3, in: $0) })
            try await shot("detail-alert-temperature-\(suffix)", strained(), scheme, act: { try await open(.temperature, on: 2, in: $0) })
            try await shot("older-watcher-\(suffix)", strained(), scheme, environment: { AnyView($0.environment(\._accessibilityShowButtonShapes, true)) }, act: { try await open(.cpu, on: 1, in: $0, opens: false) })
        }
        try await shot("detail-memory-increase-contrast", full(), .light, environment: { AnyView($0.environment(\._colorSchemeContrast, .increased)) }, act: { try await open(.memory, on: 3, in: $0) })
        try await shot("detail-memory-increase-contrast-dark", full(), .dark, environment: { AnyView($0.environment(\._colorSchemeContrast, .increased)) }, act: { try await open(.memory, on: 3, in: $0) })
        try await shot("detail-memory-show-borders", full(), .light, environment: { AnyView($0.environment(\._accessibilityShowButtonShapes, true)) }, act: { try await open(.memory, on: 3, in: $0) })
        try await shot("detail-cpu-reduce-transparency", full(), .light, opaque: true, environment: { AnyView($0.environment(\._accessibilityReduceTransparency, true)) }, act: { try await open(.cpu, on: 2, in: $0) })
        try await shot("detail-alert-differentiate-without-color", strained(), .light, environment: { AnyView($0.environment(\._accessibilityDifferentiateWithoutColor, true)) }, act: { try await open(.disk, on: 3, in: $0) })
    }

    @Test func motion() async throws {
        defer { try? FileManager.default.removeItem(at: home) }
        let full = try await store(machines(sessions: true))
        let lone = try await store(machines(sessions: true).filter { $0.key != "box" }.mapValues { $0.merging(["sessions": ($0["sessions"] as! [Any]).prefix($0["ssh"] as? String == "" ? 0 : 1).map(\.self)]) { $1 } })
        for (name, store) in [("sessions", full), ("no-sessions", lone)] {
            let window = try await panel(store, .light, height: 560, fill: .clear)
            try await film("monitor-enter-\(name)", window) { try await tap(window, at: CGPoint(x: 297, y: window.contentView!.fittingSize.height - 30)) }
            try await film("monitor-leave-\(name)", window) { try await tap(window, at: CGPoint(x: 297, y: window.contentView!.fittingSize.height - 30)) }
            window.orderOut(nil)
        }
        let window = try await panel(full, .light, height: 560, fill: .clear)
        left(window, times: 1)
        try await Task.sleep(for: .milliseconds(700))
        try await film("push", window) { try await tap(window, at: CGPoint(x: 264, y: 108)) }
        try await film("pop", window) { try await tap(window, at: CGPoint(x: 60, y: 16)) }
        window.orderOut(nil)
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
        UserDefaults.standard.set("cpu,memory,temperature,disk,network", forKey: Defaults.modules)
        let scenes = NSHostingSceneRepresentation { Settings { SettingsView(store: full, permission: Permission()) } }
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
        UserDefaults.standard.removeObject(forKey: Defaults.modules)
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

    private func shot(_ name: String, _ store: Store, _ scheme: ColorScheme, height: CGFloat = 560, opaque: Bool = false, environment: (AnyView) -> AnyView = { $0 }, act: (NSWindow) async throws -> Void = { _ in }) async throws {
        let fill = opaque ? Color(nsColor: .windowBackgroundColor) : (scheme == .light ? Color(white: 0.97, opacity: 0.94) : Color(white: 0.14, opacity: 0.94))
        let window = try await panel(store, scheme, height: height, fill: fill, environment: environment)
        try await act(window)
        try await Task.sleep(for: .milliseconds(900))
        let host = window.contentView!
        window.setContentSize(CGSize(width: host.fittingSize.width, height: min(host.fittingSize.height, height)))
        try await Task.sleep(for: .milliseconds(300))
        try capture(window, name)
        window.orderOut(nil)
    }

    private func panel(_ store: Store, _ scheme: ColorScheme, height: CGFloat, fill: Color, environment: (AnyView) -> AnyView = { $0 }) async throws -> NSWindow {
        let root = environment(AnyView(PanelView(store: store, height: height) {}))
            .background(fill, in: .rect(cornerRadius: PanelView.radius))
            .environment(\.colorScheme, scheme)
        let host = NSHostingView(rootView: root)
        let window = KeyWindow(contentRect: CGRect(origin: .zero, size: CGSize(width: host.fittingSize.width, height: fill == .clear ? height : host.fittingSize.height)), styleMask: .borderless, backing: .buffered, defer: false)
        window.level = NSWindow.Level(Int(CGWindowLevelForKey(.desktopWindow)) - 1)
        window.appearance = NSAppearance(named: scheme == .light ? .aqua : .darkAqua)
        window.backgroundColor = scheme == .light ? NSColor(white: 0.55, alpha: 1) : NSColor(white: 0.08, alpha: 1)
        if fill == .clear {
            host.sizingOptions = .intrinsicContentSize
        }
        window.contentView = host
        window.orderFrontRegardless()
        window.makeKey()
        try await Task.sleep(for: .milliseconds(300))
        return window
    }

    private func film(_ name: String, _ window: NSWindow, cue: () async throws -> Void) async throws {
        let host = window.contentView!
        try Process.run(URL(filePath: "/usr/sbin/screencapture"), arguments: ["-o", "-x", "-l", "\(window.windowNumber)", out.appending(path: "\(name).png").path])
        try await cue()
        for frame in 0..<12 {
            let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds)!
            host.cacheDisplay(in: host.bounds, to: rep)
            try rep.representation(using: .png, properties: [:])!.write(to: out.appending(path: "\(name)-\(frame).png"))
            try await Task.sleep(for: .milliseconds(40))
        }
        try await Task.sleep(for: .milliseconds(700))
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
        press(window, "\u{F703}", keyCode: 124, times: times)
    }

    private func left(_ window: NSWindow, times: Int) {
        press(window, "\u{F702}", keyCode: 123, times: times)
    }

    private func down(_ window: NSWindow, times: Int) {
        press(window, "\u{F701}", keyCode: 125, times: times)
    }

    @discardableResult private func open(_ module: Module, on machine: Int, in window: NSWindow, opens: Bool = true) async throws -> CGSize {
        right(window, times: machine)
        try await Task.sleep(for: .milliseconds(700))
        let before = window.contentView!.fittingSize
        try await tap(window, at: [Module.cpu: CGPoint(x: 96, y: 108), .memory: CGPoint(x: 264, y: 108), .temperature: CGPoint(x: 96, y: 212), .disk: CGPoint(x: 264, y: 212), .network: CGPoint(x: 180, y: 304)][module]!)
        try await Task.sleep(for: .milliseconds(700))
        #expect((window.contentView!.fittingSize != before) == opens, "\(module) on machine \(machine)")
        return before
    }

    private func tap(_ window: NSWindow, at point: CGPoint) async throws {
        let location = CGPoint(x: point.x, y: window.contentView!.bounds.height - point.y)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            window.sendEvent(NSEvent.mouseEvent(with: type, location: location, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!)
            try await Task.sleep(for: .milliseconds(90))
        }
    }

    private func press(_ window: NSWindow, _ key: String, keyCode: UInt16, times: Int) {
        for _ in 0..<times {
            window.sendEvent(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, characters: key, charactersIgnoringModifiers: key, isARepeat: false, keyCode: keyCode)!)
        }
    }

    private func store(_ machines: [String: [String: Any]]) async throws -> Store {
        let home = home.appending(path: UUID().uuidString)
        let store = Store(home: home)
        for (index, (name, machine)) in machines.sorted(by: { $0.key < $1.key }).enumerated() {
            var machine = machine
            machine["heard"] = stamp(Date.now.addingTimeInterval(-3))
            let sample = machine.removeValue(forKey: "sample") as! [String: Int]
            machine["history"] = (0..<120).map { step -> [String: Any] in
                let wave = sin(Double(step + index * 17) / 9) + 0.4 * sin(Double(step) / 2.3)
                let spike = max(0, 1 - abs(Double(step - 96)) / 6)
                var reading: [String: Any] = ["at": now.timeIntervalSince1970 - Double(119 - step) * 30]
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
        func session(_ title: String, _ folder: String, _ state: String, _ minutes: Double, cpu: Int, memory: Int, waitingOn: String? = nil, runs place: [String: Any] = [:]) -> [String: Any] {
            var session: [String: Any] = ["id": UUID().uuidString, "title": title, "folder": folder, "state": state, "since": stamp(now.addingTimeInterval(-minutes * 60)), "cpu": cpu, "memoryMB": memory]
            session["waitingOn"] = waitingOn
            return session.merging(place) { $1 }
        }
        func busy(_ pid: Int, _ name: String, _ cpu: Int, _ session: String? = nil) -> [String: Any] {
            var program: [String: Any] = ["pid": pid, "name": name, "cpu": cpu]
            program["session"] = session
            return program
        }
        func resident(_ pid: Int, _ name: String, _ memory: Int, _ session: String? = nil) -> [String: Any] {
            var program: [String: Any] = ["pid": pid, "name": name, "memoryMB": memory]
            program["session"] = session
            return program
        }
        return [
            "laptop": [
                "answering": true, "ssh": "", "cores": 10, "load": 2.4, "pressure": critical ? "warning" : "normal",
                "memory": ["usedMB": 13100, "totalMB": 16384], "disk": ["usedMB": 282192, "totalMB": 471482], "swap": ["usedMB": 1536, "totalMB": 4096],
                "bootedAt": stamp(now.addingTimeInterval(-3 * 86_400)), "battery": ["percent": critical ? 14 : 64, "onBattery": true],
                "sample": ["cpu": critical ? 61 : 18, "thermal": critical ? 1 : 0],
                "top": [
                    "cpu": [busy(4121, "claude", 34, "magi-demo"), busy(402, "WindowServer", 18), busy(9311, "Google Chrome Helper (Renderer)", 12), busy(7702, "node", 9, "shop-fix"), busy(5120, "Xcode", 6), busy(388, "mds_stores", 3), busy(611, "Finder", 1), busy(1840, "Rex", 1)],
                    "memory": [resident(811, "Google Chrome", 4915), resident(4121, "claude", 2458, "magi-demo"), resident(5120, "Xcode", 2150), resident(7702, "node", 1331, "shop-fix"), resident(402, "WindowServer", 1126), resident(2210, "Ghostty", 420), resident(1840, "Rex", 380), resident(611, "Finder", 160)],
                ],
                "memoryParts": [["kind": "app", "mb": 7800], ["kind": "wired", "mb": 2600], ["kind": "compressed", "mb": 1200], ["kind": "cached", "mb": 1500], ["kind": "free", "mb": 3284]],
                "volumes": [["name": "Macintosh HD", "mount": "/", "usedMB": 282192, "totalMB": 471482], ["name": "Time Machine", "mount": "/Volumes/Time Machine", "usedMB": critical ? 1_960_000 : 1_400_000, "totalMB": 2_000_000], ["name": "Photos", "mount": "/Volumes/Photos", "usedMB": 317_440, "totalMB": 512_000]],
                "interfaces": [["name": "en0", "rx": 2_400_000, "tx": 310_000], ["name": "utun4", "rx": 52_000, "tx": 18_000]],
                "sessions": sessions ? [
                    session("Shop checkout fix", "/Users/sam/code/shop", "waiting", 4, cpu: 0, memory: 229, waitingOn: "Allow Bash to run npm test in the shop folder?", runs: ["id": "shop-fix", "hostSession": "local_7cbf", "app": "com.anthropic.claudefordesktop"]),
                    session("magi README demo videos", "/Users/sam/code/magi", "busy", 12, cpu: 34, memory: 547, runs: ["id": "magi-demo", "app": "com.mitchellh.ghostty", "tty": "/dev/ttys004"]),
                    session("Blog drafts", "/Users/sam/code/blog", "idle", 190, cpu: 0, memory: 212, runs: ["tmux": "%3"]),
                ] : [],
            ],
            "homelab": [
                "answering": true, "ssh": "homelab", "cores": 12, "load": 3.1, "relayed": true,
                "memory": ["usedMB": 6450, "totalMB": 15384], "disk": ["usedMB": 1212000, "totalMB": 1875626], "swap": ["usedMB": 0, "totalMB": 2048],
                "bootedAt": stamp(now.addingTimeInterval(-6 * 60)),
                "sample": ["cpu": 26, "temp": critical ? 83 : 54],
                "top": [
                    "cpu": [busy(4121, "claude", 82, "shop-db"), busy(733, "postgres", 61), busy(5022, "node", 12, "backup-audit"), busy(1201, "dockerd", 6), busy(2290, "python3", 5), busy(901, "tailscaled", 2), busy(880, "sshd", 1), busy(311, "systemd-journald", 1)],
                    "memory": [resident(733, "postgres", 1434), resident(4121, "claude", 1100, "shop-db"), resident(5022, "node", 640, "backup-audit"), resident(2290, "python3", 380), resident(1201, "dockerd", 210), resident(311, "systemd-journald", 90), resident(901, "tailscaled", 60), resident(880, "sshd", 18)],
                ],
                "memoryParts": [["kind": "app", "mb": 6450], ["kind": "cached", "mb": 5200], ["kind": "free", "mb": 3734]],
                "volumes": [["name": "System", "mount": "/", "usedMB": 1_212_000, "totalMB": 1_875_626], ["name": "Backups", "mount": "/mnt/backups", "usedMB": 3_520_000, "totalMB": 3_815_447]],
                "interfaces": [["name": "eth0", "rx": 1_600_000, "tx": 310_000], ["name": "tailscale0", "rx": 420_000, "tx": 96_000], ["name": "docker0", "rx": 12_000, "tx": 3_400]],
                "sensors": [["name": "Package id 0", "temp": critical ? 83.0 : 54.0, "critical": 100.0], ["name": "Core 0", "temp": 52.0, "critical": 100.0], ["name": "Core 1", "temp": 55.0, "critical": 100.0], ["name": "nvme Composite", "temp": critical ? 78.0 : 41.9, "critical": 84.85], ["name": "acpitz", "temp": 27.8]],
                "sessions": sessions ? [
                    session("Nightly backup audit with a long title that runs out of room", "/home/sam/ops/backups/nightly", "waiting", 11, cpu: 0, memory: 310, waitingOn: "Which retention policy should I keep for the nightly snapshots, 30 days or 90 days?", runs: ["id": "backup-audit", "rex": ["session": "session:01a1", "block": "block:01a2"]]),
                    session("shop-db migration", "/home/sam/code/shop", "busy", 20, cpu: 82, memory: 1100, runs: ["id": "shop-db", "tmux": "%7"]),
                    session("infra notes", "/home/sam/notes", "idle", 1_500, cpu: 0, memory: 180, runs: ["remoteControl": "https://claude.ai/code/session_01"]),
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
