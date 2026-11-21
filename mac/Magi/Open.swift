import AppKit
import SystemConfiguration
import os

enum Host {
    case claude, browser, rex, ghostty, tmux

    var bundle: String? {
        switch self {
        case .claude: "com.anthropic.claudefordesktop"
        case .rex: "com.superlogical.rex"
        case .ghostty: "com.mitchellh.ghostty"
        case .browser, .tmux: nil
        }
    }

    var name: String {
        switch self {
        case .claude: "Claude"
        case .browser: "Browser"
        case .rex: "Rex"
        case .ghostty: "Ghostty"
        case .tmux: "tmux"
        }
    }

    var symbol: String {
        self == .browser ? "globe" : "terminal"
    }
}

enum Route: Equatable {
    case link(URL)
    case remoteControl(URL)
    case rex(ssh: String, session: String, block: String)
    case tmux(pane: String, client: String, app: String)
    case attach(pane: String, ssh: String?)
    case ghostty(tty: String)
}

extension Route {
    @MainActor var host: Host {
        switch self {
        case .link: .claude
        case .remoteControl: Apps.url(Host.claude.bundle!) == nil ? .browser : .claude
        case .rex: .rex
        case let .tmux(_, _, app): app == Host.ghostty.bundle ? .ghostty : .tmux
        case .attach: .tmux
        case .ghostty: .ghostty
        }
    }
}

extension Row {
    var route: Route? {
        if remote == nil, let hostSession = session.hostSession, let url = Script.continuing(hostSession) {
            return .link(url)
        }
        if let remote, let rex = session.rex, rex.session.hasPrefix("session:"), rex.block.hasPrefix("block:") {
            return .rex(ssh: remote, session: rex.session, block: rex.block)
        }
        if let pane = session.tmux {
            if remote == nil, let client = session.tty, let app = session.app {
                return .tmux(pane: pane, client: client, app: app)
            }
            return .attach(pane: pane, ssh: remote)
        }
        if remote == nil, session.app == Host.ghostty.bundle, let tty = session.tty {
            return .ghostty(tty: tty)
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
        ghostty(["set surface to new surface configuration", "set command of surface to \(scripted(login(command, shell: shell)))", "new window with configuration surface", "activate"])
    }

    static func ghosttyFocus(tty: String) -> [String] {
        ghostty(["focus (first terminal whose tty is \(scripted(tty)))", "activate"])
    }

    private static func ghostty(_ lines: [String]) -> [String] {
        (["tell application id \(scripted(Host.ghostty.bundle!))"] + lines + ["end tell"]).flatMap { ["-e", $0] }
    }

    static func usesGhostty(_ settings: Data?) -> Bool {
        let settings = settings.flatMap { try? JSONSerialization.jsonObject(with: $0) } as? [String: Any]
        return settings?["deepLinkTerminal"] as? String == Host.ghostty.name
    }

    static func lastLine(_ output: Data) -> String? {
        String(decoding: output, as: UTF8.self).split(separator: "\n").last.map(String.init)
    }

    static func rexLabel(ssh: String, hosts: Data) throws -> String? {
        try JSONDecoder().decode(Hosts.self, from: hosts).hosts.first { $0.host.endpoints.contains("ssh://\(ssh)") }?.host.label
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
    private static var tools: [String: Task<String, any Error>] = [:]

    static func open(_ route: Route) {
        Task {
            do {
                try await perform(route, until: .now + .seconds(10))
            } catch {
                NSSound.beep()
            }
        }
    }

    private static func perform(_ route: Route, until deadline: ContinuousClock.Instant) async throws {
        switch route {
        case let .link(url):
            try await NSWorkspace.shared.open(url, configuration: .init())
        case let .remoteControl(url):
            guard let url = Apps.url(Host.claude.bundle!) == nil ? url : Script.inClaude(url) else { throw Failure() }
            try await NSWorkspace.shared.open(url, configuration: .init())
        case let .rex(ssh, session, block):
            let rex = try await tool("rex")
            guard let label = try Script.rexLabel(ssh: ssh, hosts: await run(rex, ["hosts", "list", "--json"], until: deadline)),
                  let name = SCDynamicStoreCopyComputerName(nil, nil) as String? else { throw Failure() }
            try await run(rex, ["-S", label, "block", "focus", "-s", session, block], until: deadline)
            try await NSWorkspace.shared.openApplication(at: try required(Host.rex.bundle!), configuration: .init())
            while ContinuousClock.now < deadline {
                if let client = try Script.rexClient(await run(rex, ["-S", label, "client", "ls", "--json"], until: deadline), label: name),
                   (try? await run(rex, ["-S", label, "-C", client, "do", "session.select", "session_id=\(session)"], until: deadline)) != nil {
                    return
                }
                try await Task.sleep(for: .milliseconds(250))
            }
            throw Failure()
        case let .tmux(pane, client, app):
            try await run(try await tool("tmux"), ["switch-client", "-c", client, "-t", pane], until: deadline)
            if app == Host.ghostty.bundle {
                try await run("/usr/bin/osascript", Script.ghosttyFocus(tty: client), until: deadline)
            } else {
                try await NSWorkspace.shared.openApplication(at: try required(app), configuration: .init())
            }
        case let .attach(pane, ssh):
            try await terminal(running: Script.attach(pane: pane, ssh: ssh), until: deadline)
        case let .ghostty(tty):
            try await run("/usr/bin/osascript", Script.ghosttyFocus(tty: tty), until: deadline)
        }
    }

    private static func terminal(running command: String, until deadline: ContinuousClock.Instant) async throws {
        if Script.usesGhostty(try? Data(contentsOf: .homeDirectory.appending(path: ".claude.json"))) {
            try await run("/usr/bin/osascript", Script.ghosttyWindow(running: command, shell: loginShell), until: deadline)
            return
        }
        let script = FileManager.default.temporaryDirectory.appending(path: "magi-\(UUID().uuidString).command")
        try Data(Script.command(running: command).utf8).write(to: script)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: script.path)
        try await NSWorkspace.shared.open([script], withApplicationAt: try required("com.apple.Terminal"), configuration: .init())
    }

    private static func tool(_ name: String) async throws -> String {
        let resolved = tools[name] ?? Task {
            guard let path = Script.lastLine(try await run(loginShell, ["-l", "-c", "command -v \(name)"], until: .now + .seconds(10))) else { throw Failure() }
            return path
        }
        tools[name] = resolved
        return try await resolved.value
    }

    private static var loginShell: String {
        String(cString: getpwuid(getuid()).pointee.pw_shell)
    }

    private static func required(_ bundle: String) throws -> URL {
        guard let url = Apps.url(bundle) else { throw Failure() }
        return url
    }

    @discardableResult
    nonisolated private static func run(_ tool: String, _ arguments: [String], until deadline: ContinuousClock.Instant) async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            let pending = OSAllocatedUnfairLock<CheckedContinuation<Data, any Error>?>(initialState: continuation)
            let finish = { @Sendable (result: Result<Data, any Error>) -> Void in
                pending.withLock { $0.take() }?.resume(with: result)
            }
            let process = Process()
            process.executableURL = URL(filePath: tool)
            process.arguments = arguments
            let output = Pipe()
            process.standardOutput = output
            process.standardError = FileHandle.nullDevice
            do {
                try process.run()
            } catch {
                return finish(.failure(error))
            }
            DispatchQueue.global().asyncAfter(deadline: .now() + (deadline - .now) / .seconds(1)) {
                if process.isRunning {
                    process.terminate()
                }
                finish(.failure(Failure()))
            }
            DispatchQueue.global().async {
                let data = output.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                finish(process.terminationStatus == 0 ? .success(data) : .failure(Failure()))
            }
        }
    }

    private struct Failure: Error {}
}

@MainActor
enum Apps {
    private static var urls: [String: URL?] = [:]
    private static var icons: [String: NSImage?] = [:]

    static func url(_ bundle: String) -> URL? {
        if let url = urls[bundle] { return url }
        let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundle)
        urls[bundle] = url
        return url
    }

    static func icon(_ bundle: String) -> NSImage? {
        if let icon = icons[bundle] { return icon }
        let icon = url(bundle).map { NSWorkspace.shared.icon(forFile: $0.path) }
        icons[bundle] = icon
        return icon
    }
}
