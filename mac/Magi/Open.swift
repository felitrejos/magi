import AppKit
import SystemConfiguration

enum Host {
    case claude, rex, ghostty, terminal

    var bundle: String? {
        switch self {
        case .claude: "com.anthropic.claudefordesktop"
        case .rex: "com.superlogical.rex"
        case .ghostty: "com.mitchellh.ghostty"
        case .terminal: nil
        }
    }

    var name: String {
        switch self {
        case .claude: "Claude"
        case .rex: "Rex"
        case .ghostty: "Ghostty"
        case .terminal: "a terminal"
        }
    }
}

enum Route: Equatable {
    case link(URL)
    case remoteControl(URL)
    case rex(host: String, session: String, block: String)
    case tmux(pane: String, client: String, app: String)
    case attach(pane: String, host: String?)
    case ghostty(tty: String)
}

extension Session {
    var host: Host {
        if hostSession != nil { return .claude }
        if rex != nil { return .rex }
        return switch app {
        case Host.claude.bundle: .claude
        case "com.superlogical.rex.server": .rex
        case Host.ghostty.bundle: .ghostty
        default: .terminal
        }
    }
}

extension Row {
    var route: Route? {
        if remote == nil, let hostSession = session.hostSession {
            return .link(URL(string: "claude://code/continue?session=\(hostSession)")!)
        }
        if let remote, let rex = session.rex {
            return .rex(host: remote, session: rex.session, block: rex.block)
        }
        if let pane = session.tmux {
            if remote == nil, let client = session.tty, let app = session.app {
                return .tmux(pane: pane, client: client, app: app)
            }
            return .attach(pane: pane, host: remote)
        }
        if remote == nil, session.app == Host.ghostty.bundle, let tty = session.tty {
            return .ghostty(tty: tty)
        }
        return session.remoteControl.map(Route.remoteControl)
    }
}

@MainActor
enum Opener {
    static func open(_ route: Route) {
        Task {
            do {
                try await perform(route)
            } catch {
                NSSound.beep()
            }
        }
    }

    private static func perform(_ route: Route) async throws {
        switch route {
        case let .link(url):
            try await NSWorkspace.shared.open(url, configuration: .init())
        case let .remoteControl(url):
            var claude = URLComponents(url: url, resolvingAgainstBaseURL: false)!
            claude.scheme = "claude"
            try await NSWorkspace.shared.open(installed(Host.claude.bundle!) == nil ? url : claude.url!, configuration: .init())
        case let .rex(host, session, block):
            let rex = try await tool("rex")
            try await run(rex, "-S", host, "block", "focus", "-s", session, block)
            try await NSWorkspace.shared.openApplication(at: try required(Host.rex.bundle!), configuration: .init())
            let name = SCDynamicStoreCopyComputerName(nil, nil) as String?
            for _ in 0..<40 {
                let listed = try JSONDecoder.rex.decode(Clients.self, from: await run(rex, "-S", host, "client", "ls", "--json"))
                if let client = listed.clients.last(where: { $0.info?.kind == "app" && $0.info?.label == name && $0.connectionState == "connected" }),
                   (try? await run(rex, "-S", host, "-C", client.clientId, "do", "session.select", "session_id=\(session)")) != nil {
                    return
                }
                try await Task.sleep(for: .milliseconds(250))
            }
            throw Failure()
        case let .tmux(pane, client, app):
            try await run(try await tool("tmux"), "switch-client", "-c", client, "-t", pane)
            if app == Host.ghostty.bundle {
                try await focusGhostty(client)
            } else {
                try await NSWorkspace.shared.openApplication(at: try required(app), configuration: .init())
            }
        case let .attach(pane, host):
            let attach = "tmux attach -t \(quoted(pane))"
            try await terminal(running: host.map { "ssh -t \(quoted($0)) \(quoted(attach))" } ?? attach)
        case let .ghostty(tty):
            try await focusGhostty(tty)
        }
    }

    private static func focusGhostty(_ tty: String) async throws {
        try await run("/usr/bin/osascript", "-e", "tell application id \"\(Host.ghostty.bundle!)\"", "-e", "focus (first terminal whose tty is \(scripted(tty)))", "-e", "activate", "-e", "end tell")
    }

    private static func terminal(running command: String) async throws {
        if try lastTerminal() == "Ghostty" {
            let shell = "\(loginShell) -l -c \(quoted(command))"
            try await run("/usr/bin/osascript", "-e", "tell application id \"\(Host.ghostty.bundle!)\"", "-e", "set surface to new surface configuration", "-e", "set command of surface to \(scripted(shell))", "-e", "new window with configuration surface", "-e", "activate", "-e", "end tell")
            return
        }
        let script = FileManager.default.temporaryDirectory.appending(path: "magi-\(UUID().uuidString).command")
        try Data("#!/bin/sh\nrm -f \"$0\"\nexec \(command)\n".utf8).write(to: script)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: script.path)
        try await NSWorkspace.shared.open([script], withApplicationAt: try required("com.apple.Terminal"), configuration: .init())
    }

    private static func lastTerminal() throws -> String? {
        let settings = try JSONSerialization.jsonObject(with: Data(contentsOf: .homeDirectory.appending(path: ".claude.json"))) as? [String: Any]
        return settings?["deepLinkTerminal"] as? String
    }

    private static func tool(_ name: String) async throws -> String {
        String(decoding: try await run(loginShell, "-l", "-c", "command -v \(name)"), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static var loginShell: String {
        String(cString: getpwuid(getuid()).pointee.pw_shell)
    }

    private static func installed(_ bundle: String) -> URL? {
        NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundle)
    }

    private static func required(_ bundle: String) throws -> URL {
        guard let url = installed(bundle) else { throw Failure() }
        return url
    }

    @discardableResult
    nonisolated private static func run(_ tool: String, _ arguments: String...) async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global().async {
                let process = Process()
                process.executableURL = URL(filePath: tool)
                process.arguments = arguments
                let output = Pipe()
                process.standardOutput = output
                process.standardError = FileHandle.nullDevice
                do {
                    try process.run()
                    let data = output.fileHandleForReading.readDataToEndOfFile()
                    process.waitUntilExit()
                    guard process.terminationStatus == 0 else { throw Failure() }
                    continuation.resume(returning: data)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    private struct Failure: Error {}

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

extension Bundle {
    static let magi = Bundle(for: BundleMarker.self)
}

private final class BundleMarker {}

@MainActor
enum AppIcons {
    private static var icons: [String: NSImage] = [:]

    static func icon(_ bundle: String) -> NSImage? {
        if let icon = icons[bundle] { return icon }
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundle) else { return nil }
        let icon = NSWorkspace.shared.icon(forFile: url.path)
        icons[bundle] = icon
        return icon
    }
}
