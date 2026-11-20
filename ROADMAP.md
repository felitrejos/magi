# Roadmap

## Direction

- **Move a session, send work.** The core is moving a live Claude Code
  session to another machine and back, and sending work to other machines
  that comes back by itself. magi moves Claude Code sessions and nothing
  else.
- **One program.** `magi` is a single Go program. It needs SSH, tmux and
  the Claude Code CLI.
- **Ask, don't type.** Nobody types magi commands. Installing the plugin
  is the whole install, and everything after that is something you ask
  Claude for.
- **Reliable before new.** A handoff should be boring. Making moves safe
  and measured comes before anything else.
- **Leave it to the tools.** What Claude Code, SSH or tmux already do
  stays there.
- **Suggest, don't act.** magi suggests machines but never moves or starts
  anything by itself. The one exception is `magi watch`, which takes back
  a session you asked to hand back. Beyond that it only cleans up after
  itself.
- **A layer for other tools.** `magi mcp` gives Claude and any other MCP
  client magi's actions. No tool types into another live session. You
  answer a session in its own app.

## 1.0

The core is built. What it still can't do is listed under Known limits in
[docs/support.md](docs/support.md). The one thing left for 1.0 is the menu
bar app and shipping it.

### The menu bar app

#### Goal and bar

A native, read-only Mac menu bar app for magi. It shows every machine and
every Claude Code session at a glance, says what is waiting on you, and
opens a session where it lives. It never moves or starts anything and
never types into a session.

The bar for polish is Henrik Ruscon's apps,
[Alcove](https://tryalcove.com), [Klack](https://tryklack.com) and
[Canapé](https://trycanape.com). The app runs on macOS 26 and later, and
uses macOS 27 APIs where they are better. It uses public APIs only.

#### Architecture

- **Host.** Swift 6, an AppKit host with SwiftUI views. An `NSStatusItem`
  whose button holds an `NSImageView`, so the icon can use SF Symbol
  effects, which `NSStatusBarButton` doesn't have. The item opens a
  borderless, non-activating `NSPanel` under it that hosts SwiftUI through
  `NSHostingView`.
- **Opening the panel.** On macOS 27 the app uses
  [`NSStatusItem.expandedInterfaceDelegate`](https://developer.apple.com/documentation/appkit/nsstatusitem/expandedinterfacedelegate)
  and `NSStatusItemExpandedInterfaceSession`, so the system draws the
  highlight and ends the session. On macOS 26 a click handler does it.
- **Not `MenuBarExtra`.** SwiftUI's
  [`MenuBarExtra`](https://developer.apple.com/documentation/swiftui/menubarextra)
  can't be opened from code, has a static label, gives no access to the
  status item and has known bugs in its `.window` style.
- **The panel behaves like a menu.** It opens instantly and closes on a
  click outside or on Escape. The keyboard does everything. The arrows
  move the selection, Return opens the selected session and Command-comma
  opens Settings. A right-click on the icon gives a small menu with
  Settings and Quit.
- **Settings.** A SwiftUI `Settings` scene hosted from AppKit with
  `NSHostingSceneRepresentation`, as shown in
  [WWDC26 session 272](https://developer.apple.com/videos/play/wwdc2026/272/).
  It has four panes.
  - General has launch at login through `SMAppService.mainApp`, the
    waiting count in the bar, and sounds.
  - Notifications.
  - Machines, a read-only list.
  - About and updates.
- **Data.** One `@Observable` store, fed by a `DispatchSource` watch on
  the `~/.magi/live` directory. When a file changes the store reads that
  file again. magi writes by atomic rename, and
  [`NSFilePresenter`](https://developer.apple.com/documentation/foundation/nsfilepresenter)
  would miss those writes. The store also reads `~/.magi/handoffs.jsonl`
  for recent moves.
- **Idle until a file changes.** The app never polls, never runs SSH and
  never runs magi.
- **Not answering.** A machine whose `heard` is older than about 40
  seconds, or whose `answering` is false, shows as not answering.
- **Motion.** Durations use `Text(.timer)` or `TimelineView`. Numbers that
  change use `.contentTransition(.numericText())`. State changes use SF
  Symbol effects. All of it honours Reduce Motion.
- **Materials.** Rows sit on standard materials with vibrancy. Liquid
  Glass goes on the one primary control, if there is one, as the
  [HIG](https://developer.apple.com/design/human-interface-guidelines/materials)
  says. Test with the transparent menu bar, the macOS 27 glass tint
  slider, Reduce Transparency and Increase Contrast.

#### What magi adds for the app

- **Per session.** The live file gains the Remote Control link and the
  tmux session name. The watcher already streams the registry line with
  `bridgeSessionId`.
- **Per machine.** The live file gains the machine's SSH destination.
- **Why.** With these the app needs no TOML parser and no SSH of its own.
- **A stable surface.** [docs/versioning.md](docs/versioning.md) lists the
  live file and `handoffs.jsonl` as the stable surface the app reads.

#### The panel

The panel is calm and puts content first.

- **Waiting on you.** This group comes first whenever a session is
  waiting. Each row carries Claude's own wording of what it waits for.
- **One group per machine.** The header has the machine's name, a dot for
  answering, and compact gauges for load against cores, memory and disk.
- **Sessions.** Under each machine, one row per session with its title,
  short folder, state (busy, idle or waiting), how long it has been in
  that state, and its CPU and memory.
- **A machine that doesn't answer stays.** It is dimmed and says when it
  was last heard.
- **Footer.** The last few moves from `handoffs.jsonl`.
- **Look.** The selection is a soft pill. Groups are separated by tone and
  space instead of lines. Digits are rounded or monospaced so numbers
  don't jitter.

#### Opening a session

- **Return or a click** opens the session's Remote Control link on
  claude.ai, in the default browser or the Claude app.
- **A secondary action** opens it in a terminal with
  `ssh -t <host> tmux attach -t =<name>`, in the terminal app you chose in
  Settings. It goes through a temporary `.command` file or the terminal's
  own URL scheme, so the app needs no Apple Events permission.

#### Notifications

- **When a session starts waiting**, a time-sensitive notification names
  the machine, the session and what it waits for, with an Open action. It
  uses UserNotifications. Time-sensitive breaks through Focus, and you can
  turn that off.
- **Sounds.** An optional sound per event, off by default, the way Klack
  does it.
- **A second way in.** When the notch hides the icon, a notification still
  reaches you.

#### The menu bar icon

One custom template glyph, drawn as a custom SF Symbol with Draw
annotations, so it matches the bar's weight and can animate.
[SF Symbols 8](https://developer.apple.com/sf-symbols/) ships with the 27
OSes.

| State | What the icon does |
|---|---|
| Idle | Shows the glyph |
| Busy | Shows the glyph and doesn't animate |
| Waiting | Adds a badge, and plays a one-shot Draw On effect when a session starts waiting |
| No machine answers | Dims the glyph to about 35% |

- **The badge** is a template cutout by default. A coloured accent dot is
  opt-in, since colour breaks template tinting.
- **The waiting count** next to the glyph is optional and off by default,
  because it changes the item's width.
- **Accessibility.** An accessibility description always says the state.
- **The app icon** is separate. It is an
  [Icon Composer](https://developer.apple.com/icon-composer/) `.icon` with
  default, dark, clear and tinted appearances.

#### First run

A short wizard with one step per prerequisite. You can skip every step and
run it again from Settings. Henrik's changelogs show that users get stuck
on permission steps they can't skip.

1. Is magi set up here? The live files exist and are fresh. If not, the
   step says to ask Claude "set up magi with my homelab".
2. Notifications.
3. Launch at login.

#### Budget

Idle CPU near 0%, since the app only wakes on a file change, and under
40 MB of memory. Both are measured and published, like Henrik does for his
apps.

#### Engineering

- An Xcode project in its own folder of the repo.
- Swift Testing for the store and for the parsing of live files and
  `handoffs.jsonl`.
- A macOS CI job that builds and tests it.
- The same standards as the Go code. No comments, no dead code, one test
  per behaviour.

### Shipping the menu bar app

- **Signing.** Signed with Developer ID, with the Hardened Runtime, in a
  [notarized](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
  and stapled DMG.
- **Unsandboxed.** The app reads `~/.magi` and opens terminals.
- **Updates.** Sparkle 2 with an EdDSA-signed appcast, plus a cask in the
  project's own Homebrew tap.
- **Why notarize.** The old plan to ship unnotarized no longer works.
  Since macOS Sequoia,
  [Control-click and Open doesn't bypass Gatekeeper](https://developer.apple.com/news/?id=saqachfa).
  [Homebrew 6.0](https://brew.sh/2026/06/11/homebrew-6.0.0/) said casks
  that fail Gatekeeper would be disabled from September 2026.
- **XProtect.** Test on macOS 27 that XProtect doesn't block the app from
  reading files that another team's binary wrote.
- **Open, the owner's decision.** Notarizing needs the Apple Developer
  Program, which costs US$99 a year.

### Order of work

1. magi's additions to the live file.
2. The skeleton. Status item, panel, store and real data, with no styling.
3. The design pass. Layout, the icon and its states, motion, Settings and
   first run.
4. Notifications and opening sessions.
5. Shipping. Signing, notarization, Sparkle and the tap.
6. A council review, a week of real use, and the measured budget.

### Not in 1.0

- Moving cloud sessions.
- GitHub or pull request workflows.
- Moving or starting sessions by itself.
- Native Windows and WSL.
- New ways to delegate work.

## Decided against

- **Actions in the menu bar.** Answering prompts or typing into a session
  from the menu bar would duplicate the session's own app and need hooks
  that wait for an answer. Opening the session is enough.
- **Tools that type into other sessions.** Remote Control already lets you
  answer a session yourself.
- **A `MenuBarExtra`-only app.** It can't be opened from code, its label
  is static and it hides the status item.
- **Private APIs.** The app uses public APIs only.
- **Polling SSH from the app.** `magi watch` already holds one connection
  per machine and writes what it hears to the live files.
- **Hiding the menu bar icon.** The icon is the app.
