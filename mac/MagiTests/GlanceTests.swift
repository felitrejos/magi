import Testing

struct GlanceTests {
    let id = "laptop/07f37932-4bda-4e5a-a489-719326b0f477"
    let question = Waiting(title: "Shop checkout fix", machine: "laptop", folder: "shop", on: "Allow Bash to run npm test?")

    @Test func notifiesOncePerChangeToWaitingAndWithdrawsItWhenTheWaitEnds() throws {
        let launch = glance([id: question])
        #expect(launch.started(after: nil).isEmpty && launch.asking(after: nil).isEmpty)
        #expect(launch.stale([id, "box/ee27a88a"]) == ["box/ee27a88a"])

        let idle = glance([:])
        #expect(idle.started(after: launch).isEmpty && idle.stopped(after: launch) == [id])

        let waiting = glance([id: question])
        let started = waiting.started(after: idle)
        #expect(Array(started.keys) == [id] && waiting.stopped(after: idle).isEmpty)
        let content = try #require(started[id]?.content)
        #expect([content.title, content.subtitle, content.body] == ["Shop checkout fix", "laptop \u{00B7} shop", "Allow Bash to run npm test?"])

        let still = glance([id: question])
        #expect(still.started(after: waiting).isEmpty && still.asking(after: waiting).isEmpty && still.stopped(after: waiting).isEmpty)

        let asked = glance([id: Waiting(title: question.title, machine: question.machine, folder: question.folder, on: "Allow Edit of package.json?")])
        #expect(asked.started(after: still).isEmpty && asked.asking(after: still) == [id] && asked.stopped(after: still).isEmpty)

        let gone = glance([:])
        #expect(gone.started(after: asked).isEmpty && gone.asking(after: asked).isEmpty && gone.stopped(after: asked) == [id])
    }

    private func glance(_ waiting: [String: Waiting]) -> Glance {
        Glance(waiting: waiting, answering: ["laptop"], hasMachines: true)
    }
}
