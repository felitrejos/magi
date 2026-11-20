import AppKit
import SwiftUI
import Testing

@MainActor
@Suite(.enabled(if: ProcessInfo.processInfo.environment["SNAPSHOTS"] != nil), .serialized)
struct Snapshots {
    let out = URL(filePath: ProcessInfo.processInfo.environment["SNAPSHOTS"] ?? "")
    let now = Date.now

    @Test func panel() async throws {
        let full = try await store(machines(sessions: true))
        let quiet = try await store(machines(sessions: false))
        let bare = try await store([:])
        for scheme in [ColorScheme.light, .dark] {
            let suffix = scheme == .light ? "light" : "dark"
            try await shot("main-\(suffix)", full, scheme)
            try await shot("waiting-row-\(suffix)", full, scheme, act: { window in
                window.sendEvent(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: window.windowNumber, context: nil, characters: "\u{F701}", charactersIgnoringModifiers: "\u{F701}", isARepeat: false, keyCode: 125)!)
            })
            try await shot("scope-\(suffix)", full, scheme, act: { right($0, times: 2) })
            try await shot("empty-\(suffix)", quiet, scheme)
            try await shot("not-set-up-\(suffix)", bare, scheme)
        }
        try await shot("reduce-motion", full, .light, environment: { AnyView($0.environment(\._accessibilityReduceMotion, true)) })
        try await shot("reduce-transparency", full, .light, opaque: true, environment: { AnyView($0.environment(\._accessibilityReduceTransparency, true)) })
        try await shot("increase-contrast", full, .light, environment: { AnyView($0.environment(\._colorSchemeContrast, .increased)) })
        try await shot("increase-contrast-dark", full, .dark, environment: { AnyView($0.environment(\._colorSchemeContrast, .increased)) })
        try await shot("show-borders", full, .light, environment: { AnyView($0.environment(\._accessibilityShowButtonShapes, true)) })
        try await shot("show-borders-scope", full, .light, environment: { AnyView($0.environment(\._accessibilityShowButtonShapes, true)) }, act: { right($0, times: 2) })
    }

    @Test func glyphs() throws {
        let bundle = Bundle(for: Marker.self)
        let candidates = ["trio", "orbit", "bridge"]
        let config = NSImage.SymbolConfiguration(textStyle: .body, scale: .large)
        let scale: CGFloat = 4
        let cell = CGSize(width: 44, height: 24)
        for dark in [false, true] {
            let size = CGSize(width: cell.width * 5 + 80, height: cell.height * CGFloat(candidates.count))
            let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale), pixelsHigh: Int(size.height * scale), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
            rep.size = size
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
            (dark ? NSColor(white: 0.17, alpha: 1) : NSColor(white: 0.93, alpha: 1)).setFill()
            NSRect(origin: .zero, size: size).fill()
            let ink = dark ? NSColor.white : NSColor.black
            let tinted = config.applying(.init(paletteColors: [ink]))
            for (row, name) in candidates.enumerated() {
                let y = size.height - CGFloat(row + 1) * cell.height
                NSAttributedString(string: name, attributes: [.font: NSFont.systemFont(ofSize: 9), .foregroundColor: ink.withAlphaComponent(0.6)]).draw(at: CGPoint(x: 6, y: y + 7))
                let symbols = [name, "\(name).badge", "\(name).slash"].map { NSImage(symbolName: $0, bundle: bundle, variableValue: 0)! } + ["wifi", "battery.75percent"].map { NSImage(systemSymbolName: $0, accessibilityDescription: nil)! }
                for (column, symbol) in symbols.enumerated() {
                    let image = symbol.withSymbolConfiguration(tinted)!
                    let x = 80 + CGFloat(column) * cell.width + (cell.width - image.size.width) / 2
                    image.draw(in: CGRect(x: x, y: y + (cell.height - image.size.height) / 2, width: image.size.width, height: image.size.height))
                }
            }
            NSGraphicsContext.restoreGraphicsState()
            try rep.representation(using: .png, properties: [:])!.write(to: out.appending(path: "glyphs-\(dark ? "dark" : "light").png"))
        }
    }

    private func shot(_ name: String, _ store: Store, _ scheme: ColorScheme, opaque: Bool = false, environment: (AnyView) -> AnyView = { $0 }, act: (NSWindow) -> Void = { _ in }) async throws {
        let fill = opaque ? Color(nsColor: .windowBackgroundColor) : (scheme == .light ? Color(white: 0.97, opacity: 0.94) : Color(white: 0.14, opacity: 0.94))
        let root = environment(AnyView(PanelView(store: store) {}))
            .background(fill, in: .rect(cornerRadius: PanelView.radius))
            .environment(\.colorScheme, scheme)
        let host = NSHostingView(rootView: root)
        let window = KeyWindow(contentRect: CGRect(origin: CGPoint(x: -4000, y: -4000), size: host.fittingSize), styleMask: .borderless, backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: scheme == .light ? .aqua : .darkAqua)
        window.backgroundColor = scheme == .light ? NSColor(white: 0.55, alpha: 1) : NSColor(white: 0.08, alpha: 1)
        window.contentView = host
        window.orderFrontRegardless()
        window.makeKey()
        try await Task.sleep(for: .milliseconds(300))
        act(window)
        try await Task.sleep(for: .milliseconds(900))
        window.setContentSize(CGSize(width: host.fittingSize.width, height: min(host.fittingSize.height, 560)))
        try await Task.sleep(for: .milliseconds(300))
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
        let home = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        let store = Store(home: home)
        for (name, machine) in machines {
            for step in (0..<45).reversed() where name == "homelab" || step == 0 {
                var machine = machine
                let wave = sin(Double(step) / 5)
                machine["heard"] = stamp(now.addingTimeInterval(-Double(step) * 30 - 3))
                if step > 0 {
                    machine["load"] = max(0.2, 3.4 + 2.6 * wave)
                    machine["sessions"] = (machine["sessions"] as! [[String: Any]]).enumerated().map { index, session in
                        var session = session
                        session["cpu"] = Int(max(0, 40 + 38 * sin(Double(step + index * 7) / 3)))
                        if session["state"] as? String == "waiting", step > 22 {
                            session["state"] = "busy"
                        }
                        return session
                    }
                }
                let data = try JSONSerialization.data(withJSONObject: machine)
                let temp = home.appending(path: "live/\(name).json.1")
                try data.write(to: temp)
                rename(temp.path, home.appending(path: "live/\(name).json").path)
                let heard = try JSONDecoder.magi.decode(Machine.self, from: data).heard
                for await met in Observations({ store.machines[name]?.heard == heard }) where met {
                    break
                }
            }
        }
        let moves = [
            #"{"time":"\#(stamp(now.addingTimeInterval(-3_400)))","kind":"push","from":"laptop","to":"homelab","to_dir":"/home/sam/code/shop","bytes":1240000000,"seconds":26}"#,
            #"{"time":"\#(stamp(now.addingTimeInterval(-900)))","kind":"pull","from":"homelab","to":"laptop","to_dir":"/Users/sam/code/blog","bytes":38000000,"seconds":2.1}"#,
        ]
        try Data(moves.joined(separator: "\n").appending("\n").utf8).write(to: home.appending(path: "handoffs.jsonl"))
        try await Task.sleep(for: .milliseconds(200))
        return store
    }

    private func machines(sessions: Bool) -> [String: [String: Any]] {
        func session(_ title: String, _ folder: String, _ state: String, _ minutes: Double, cpu: Int, memory: Int, waitingOn: String? = nil) -> [String: Any] {
            var session: [String: Any] = ["id": UUID().uuidString, "title": title, "folder": folder, "state": state, "since": stamp(now.addingTimeInterval(-minutes * 60)), "cpu": cpu, "memoryMB": memory]
            session["waitingOn"] = waitingOn
            return session
        }
        return [
            "laptop": [
                "answering": true, "ssh": "", "cores": 10, "load": 2.4, "pressure": "warning",
                "memory": ["usedMB": 13100, "totalMB": 16384], "disk": ["usedMB": 282192, "totalMB": 471482], "swap": ["usedMB": 1536, "totalMB": 4096],
                "bootedAt": stamp(now.addingTimeInterval(-3 * 86_400)), "battery": ["percent": 64, "charging": false, "onBattery": true],
                "sessions": sessions ? [
                    session("Shop checkout fix", "/Users/sam/code/shop", "waiting", 4, cpu: 0, memory: 229, waitingOn: "Allow Bash to run npm test in the shop folder?"),
                    session("magi README demo videos", "/Users/sam/code/magi", "busy", 12, cpu: 34, memory: 547),
                    session("Blog drafts", "/Users/sam/code/blog", "idle", 190, cpu: 0, memory: 212),
                ] : [],
            ],
            "homelab": [
                "answering": true, "ssh": "homelab", "cores": 12, "load": 3.1, "pressure": "normal", "relayed": true,
                "memory": ["usedMB": 6450, "totalMB": 15384], "disk": ["usedMB": 1712000, "totalMB": 1875626], "swap": ["usedMB": 0, "totalMB": 2048],
                "bootedAt": stamp(now.addingTimeInterval(-6 * 60)),
                "sessions": sessions ? [
                    session("Nightly backup audit with a long title that runs out of room", "/home/sam/ops/backups/nightly", "waiting", 11, cpu: 0, memory: 310, waitingOn: "Which retention policy should I keep for the nightly snapshots, 30 days or 90 days?"),
                    session("shop-db migration", "/home/sam/code/shop", "busy", 20, cpu: 82, memory: 1100),
                    session("infra notes", "/home/sam/notes", "idle", 1_500, cpu: 0, memory: 180),
                ] : [],
            ],
            "box": [
                "answering": false, "ssh": "box", "cores": 8, "load": 0.1,
                "memory": ["usedMB": 1698, "totalMB": 15384], "disk": ["usedMB": 24380, "totalMB": 975626],
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
