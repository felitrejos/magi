import AppKit
import SystemConfiguration
import os

enum BundleID {
    static let claude = "com.anthropic.claudefordesktop"
    static let rex = "com.superlogical.rex"
    static let ghostty = "com.mitchellh.ghostty"
    static let terminal = "com.apple.Terminal"
}

enum Host {
    case claude, browser, rex, ghostty, tmux, terminal

    var bundle: String? {
        switch self {
        case .claude: BundleID.claude
        case .rex: BundleID.rex
        case .ghostty: BundleID.ghostty
        case .browser, .tmux, .terminal: nil
        }
    }

    var name: String {
        switch self {
        case .claude: "Claude"
        case .browser: "Browser"
        case .rex: "Rex"
        case .ghostty: "Ghostty"
        case .tmux: "tmux"
        case .terminal: "Terminal"
        }
    }

    var symbol: String {
        self == .browser ? "globe" : "terminal"
    }

    @MainActor func iconFile() throws -> URL {
        let side = 256.0
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(side), pixelsHigh: Int(side), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        if let icon = bundle.flatMap(Apps.icon) {
            icon.draw(in: NSRect(x: 0, y: 0, width: side, height: side))
        } else {
            let glyph = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)!.withSymbolConfiguration(.init(pointSize: side / 2, weight: .medium).applying(.init(paletteColors: [.systemGray])))!
            glyph.draw(in: NSRect(x: (side - glyph.size.width) / 2, y: (side - glyph.size.height) / 2, width: glyph.size.width, height: glyph.size.height))
        }
        NSGraphicsContext.restoreGraphicsState()
        let file = FileManager.default.temporaryDirectory.appending(path: "magi-\(UUID().uuidString).png")
        try rep.representation(using: .png, properties: [:])!.write(to: file)
        return file
    }
}

enum Route: Equatable {
    case link(URL)
    case remoteControl(URL)
    case rex(ssh: String, session: String, block: String)
    case terminal(pane: String?, tty: String, app: String)
    case attach(pane: String, ssh: String?)
}

extension Route {
    @MainActor var host: Host {
        switch self {
        case .link: .claude
        case .remoteControl: Apps.url(BundleID.claude) == nil ? .browser : .claude
        case .rex: .rex
        case let .terminal(pane, _, app): app == BundleID.ghostty ? .ghostty : pane == nil ? .terminal : .tmux
        case .attach: .tmux
        }
    }
}

extension Row {
    @MainActor var route: Route? {
        route(rexInstalled: Apps.url(BundleID.rex) != nil)
    }

    func route(rexInstalled: Bool) -> Route? {
        if remote == nil, let hostSession = session.hostSession, let url = Script.continuing(hostSession) {
            return .link(url)
        }
        if rexInstalled, let remote, let rex = session.rex, rex.session.hasPrefix("session:"), rex.block.hasPrefix("block:") {
            return .rex(ssh: remote, session: rex.session, block: rex.block)
        }
        if let pane = session.tmux {
            if remote == nil, let tty = session.tty, let app = session.app {
                return .terminal(pane: pane, tty: tty, app: app)
            }
            return .attach(pane: pane, ssh: remote)
        }
        if remote == nil, let tty = session.tty, let app = session.app, Script.focus(tty: tty, in: app) != nil {
            return .terminal(pane: nil, tty: tty, app: app)
        }
        return session.remoteControl.map(Route.remoteControl)
    }
}

enum Script {
    static func continuing(_ session: String) -> URL? {
        var components = URLComponents()
        components.scheme = "claude"
        components.host = "code"
        components.path = "/continue"
        components.queryItems = [URLQueryItem(name: "session", value: session)]
        return components.url
    }

    static func inClaude(_ url: URL) -> URL? {
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        components?.scheme = "claude"
        return components?.url
    }

    static func attach(pane: String, ssh: String?) -> String {
        let attach = "tmux attach -t \(quoted(pane))"
        return ssh.map { "ssh -t -- \(quoted($0)) \(quoted(attach))" } ?? attach
    }

    static func command(running command: String) -> String {
        "#!/bin/sh\nrm -f \"$0\"\nexec \(command)\n"
    }

    static func login(_ command: String, shell: String) -> String {
        "\(quoted(shell)) -l -c \(quoted(command))"
    }

    static func ghosttyWindow(running command: String, shell: String) -> [String] {
        tell(BundleID.ghostty, ["set surface to new surface configuration", "set command of surface to \(scripted(login(command, shell: shell)))", "new window with configuration surface"])
    }

    static func focus(tty: String, in app: String) -> [String]? {
        switch app {
        case BundleID.ghostty: ghosttyFocus(tty: tty)
        case BundleID.terminal: terminalFocus(tty: tty)
        default: nil
        }
    }

    static func ghosttyFocus(tty: String) -> [String] {
        tell(BundleID.ghostty, ["focus (first terminal whose tty is \(scripted(tty)))"])
    }

    static func terminalFocus(tty: String) -> [String] {
        tell(BundleID.terminal, [
            "repeat with w in windows",
            "repeat with t in tabs of w",
            "if tty of t is \(scripted(tty)) then",
            "set selected of t to true",
            "set index of w to 1",
            "return",
            "end if",
            "end repeat",
            "end repeat",
        ])
    }

    private static func tell(_ app: String, _ lines: [String]) -> [String] {
        (["tell application id \(scripted(app))"] + lines + ["end tell"]).flatMap { ["-e", $0] }
    }

    static func usesGhostty(_ settings: Data?) -> Bool {
        let settings = settings.flatMap { try? JSONSerialization.jsonObject(with: $0) } as? [String: Any]
        return settings?["deepLinkTerminal"] as? String == "Ghostty"
    }

    static func rexLabel(ssh: String, hosts: Data) throws -> String? {
        let machine = hostName(ssh)
        return try JSONDecoder().decode(Hosts.self, from: hosts).hosts.first { $0.host.endpoints.contains { $0.hasPrefix("ssh://") && hostName(String($0.dropFirst(6))) == machine } }?.host.label
    }

    private static func hostName(_ destination: String) -> Substring? {
        destination.split(separator: "@").last
    }

    static func rexClient(_ clients: Data, label: String) throws -> String? {
        try JSONDecoder.rex.decode(Clients.self, from: clients).clients.last { $0.info?.kind == "app" && $0.info?.label == label && $0.connectionState == "connected" }?.clientId
    }

    private struct Hosts: Decodable {
        let hosts: [Entry]

        struct Entry: Decodable {
            let host: Server
        }

        struct Server: Decodable {
            let label: String
            let endpoints: [String]
        }
    }

    private struct Clients: Decodable {
        let clients: [Client]

        struct Client: Decodable {
            let clientId: String
            let info: Info?
            let connectionState: String
        }

        struct Info: Decodable {
            let kind: String
            let label: String?
        }
    }
}

func quoted(_ text: String) -> String {
    "'" + text.replacingOccurrences(of: "'", with: "'\\''") + "'"
}

func scripted(_ text: String) -> String {
    "\"" + text.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
}

private extension JSONDecoder {
    static let rex = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()
}

@MainActor
enum Opener {
    private static let log = Logger(subsystem: "io.github.felitrejos.magi", category: "open")
    private static var tools: [String: Task<String, any Error>] = [:]

    static func open(_ route: Route) {
        Task {
            do {
                try await perform(route, until: .now + .seconds(10))
            } catch {
                log.error("Opening \(String(describing: route), privacy: .public) failed: \(String(describing: error), privacy: .public)")
                NSSound.beep()
            }
        }
    }

    private static func perform(_ route: Route, until deadline: ContinuousClock.Instant) async throws {
        switch route {
        case let .link(url):
            try await NSWorkspace.shared.open(url, configuration: .init())
        case let .remoteControl(url):
            guard let url = route.host == .claude ? Script.inClaude(url) : url else { throw Failure.notFound("a claude link for \(url)") }
            try await NSWorkspace.shared.open(url, configuration: .init())
        case let .rex(ssh, session, block):
            let rex = try await tool("rex", until: deadline)
            guard let label = try Script.rexLabel(ssh: ssh, hosts: await run(rex, ["hosts", "list", "--json"], until: deadline)) else { throw Failure.notFound("a Rex host for \(ssh)") }
            guard let name = SCDynamicStoreCopyComputerName(nil, nil) as String? else { throw Failure.notFound("this Mac's name") }
            try await NSWorkspace.shared.openApplication(at: try required(BundleID.rex), configuration: .init())
            var failure: any Error = Failure.notFound("\(name) among the Rex clients of \(label)")
            while ContinuousClock.now < deadline {
                if let client = try Script.rexClient(await run(rex, ["-S", label, "client", "ls", "--json"], until: deadline), label: name) {
                    do {
                        try await run(rex, ["-S", label, "-C", client, "do", "session.select", "session_id=\(session)"], until: deadline)
                    } catch {
                        failure = error
                        try await Task.sleep(for: .milliseconds(250))
                        continue
                    }
                    try await run(rex, ["-S", label, "block", "focus", "-s", session, block], until: deadline)
                    return
                }
                try await Task.sleep(for: .milliseconds(250))
            }
            throw failure
        case let .terminal(pane, tty, app):
            if let pane {
                try await run(try await tool("tmux", until: deadline), ["switch-client", "-c", tty, "-t", pane], until: deadline)
            }
            if let focus = Script.focus(tty: tty, in: app) {
                try await run("/usr/bin/osascript", focus, until: deadline)
            }
            try await NSWorkspace.shared.openApplication(at: try required(app), configuration: .init())
        case let .attach(pane, ssh):
            try await terminal(running: Script.attach(pane: pane, ssh: ssh), until: deadline)
        }
    }

    private static func terminal(running command: String, until deadline: ContinuousClock.Instant) async throws {
        if Script.usesGhostty(try? Data(contentsOf: .homeDirectory.appending(path: ".claude.json"))) {
            try await run("/usr/bin/osascript", Script.ghosttyWindow(running: command, shell: loginShell), until: deadline)
            try await NSWorkspace.shared.openApplication(at: try required(BundleID.ghostty), configuration: .init())
            return
        }
        let script = FileManager.default.temporaryDirectory.appending(path: "magi-\(UUID().uuidString).command")
        try Data(Script.command(running: command).utf8).write(to: script)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: script.path)
        try await NSWorkspace.shared.open([script], withApplicationAt: try required(BundleID.terminal), configuration: .init())
    }

    private static func tool(_ name: String, until deadline: ContinuousClock.Instant) async throws -> String {
        let lookup = tools[name] ?? Task {
            guard let path = String(decoding: try await run(loginShell, ["-l", "-c", "command -v \(name)"], until: deadline), as: UTF8.self).split(separator: "\n").last else { throw Failure.notFound(name) }
            return String(path)
        }
        tools[name] = lookup
        do {
            return try await lookup.value
        } catch {
            tools[name] = nil
            throw error
        }
    }

    private static var loginShell: String {
        String(cString: getpwuid(getuid()).pointee.pw_shell)
    }

    private static func required(_ bundle: String) throws -> URL {
        guard let url = Apps.url(bundle) else { throw Failure.notFound(bundle) }
        return url
    }

    @discardableResult
    nonisolated static func run(_ tool: String, _ arguments: [String], until deadline: ContinuousClock.Instant) async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            let pending = OSAllocatedUnfairLock<CheckedContinuation<Data, any Error>?>(initialState: continuation)
            let finish = { @Sendable (result: Result<Data, any Error>) -> Void in
                pending.withLock { $0.take() }?.resume(with: result)
            }
            let command = [tool] + arguments
            let process = Process()
            process.executableURL = URL(filePath: tool)
            process.arguments = arguments
            let output = Pipe()
            let errors = Pipe()
            process.standardOutput = output
            process.standardError = errors
            let ended = DispatchGroup()
            ended.enter()
            process.terminationHandler = { _ in ended.leave() }
            do {
                try process.run()
            } catch {
                process.terminationHandler = nil
                ended.leave()
                return finish(.failure(error))
            }
            let collect = { @Sendable (pipe: Pipe) -> (data: @Sendable () -> Data, stop: @Sendable () -> Void) in
                let state = OSAllocatedUnfairLock(initialState: (data: Data(), open: true))
                let stop = { @Sendable in
                    guard state.withLock({ state in defer { state.open = false }; return state.open }) else { return }
                    pipe.fileHandleForReading.readabilityHandler = nil
                    ended.leave()
                }
                ended.enter()
                pipe.fileHandleForReading.readabilityHandler = { handle in
                    let chunk = handle.availableData
                    guard chunk.isEmpty else { return state.withLock { $0.data.append(chunk) } }
                    stop()
                }
                return ({ state.withLock(\.data) }, stop)
            }
            let out = collect(output)
            let said = collect(errors)
            DispatchQueue.global().asyncAfter(deadline: .now() + (deadline - .now) / .seconds(1)) {
                if process.isRunning {
                    process.terminate()
                }
                finish(.failure(Failure.timedOut(command: command)))
                out.stop()
                said.stop()
            }
            ended.notify(queue: .global()) {
                guard process.terminationStatus != 0 else { return finish(.success(out.data())) }
                finish(.failure(Failure.exited(command: command, status: process.terminationStatus, stderr: String(decoding: said.data(), as: UTF8.self))))
            }
        }
    }

    enum Failure: Error, Equatable {
        case notFound(String)
        case exited(command: [String], status: Int32, stderr: String)
        case timedOut(command: [String])
    }
}

@MainActor
enum Apps {
    private static var urls: [String: URL?] = [:]
    private static var icons: [String: NSImage] = [:]

    static func url(_ bundle: String) -> URL? {
        if let url = urls[bundle] { return url }
        let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundle)
        urls[bundle] = .some(url)
        return url
    }

    static func icon(_ bundle: String) -> NSImage? {
        if let icon = icons[bundle] { return icon }
        let icon = url(bundle).map { NSWorkspace.shared.icon(forFile: $0.path) }
        icons[bundle] = icon
        return icon
    }

    static func forgetMissing() {
        urls = urls.filter { $0.value != nil }
    }
}
