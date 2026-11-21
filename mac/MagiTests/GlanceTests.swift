import Testing

struct GlanceTests {
    let id = "laptop/07f37932-4bda-4e5a-a489-719326b0f477"
    let question = Waiting(title: "Shop checkout fix", machine: "laptop", folder: "shop", on: "Allow Bash to run npm test?")

    @Test func notifiesOncePerChangeToWaitingAndWithdrawsItWhenTheWaitEnds() throws {
        let launch = glance([id: question])
        #expect(launch.started(after: nil).isEmpty)

        let idle = glance([:])
        #expect(idle.started(after: launch).isEmpty && idle.stopped(after: launch) == [id])

        let waiting = glance([id: question])
        let started = waiting.started(after: idle)
        #expect(Array(started.keys) == [id] && waiting.stopped(after: idle).isEmpty)
        let content = try #require(started[id]?.content)
        #expect([content.title, content.subtitle, content.body] == ["Shop checkout fix", "laptop \u{00B7} shop", "Allow Bash to run npm test?"])

        let still = glance([id: question])
        #expect(still.started(after: waiting).isEmpty && still.stopped(after: waiting).isEmpty)

        let gone = glance([:])
        #expect(gone.started(after: still).isEmpty && gone.stopped(after: still) == [id])
    }

    @Test func withdrawsNotificationsFromBeforeLaunchUnlessTheSessionStillWaits() {
        #expect(glance([id: question]).stale([id, "box/ee27a88a"]) == ["box/ee27a88a"])
    }

    private func glance(_ waiting: [String: Waiting]) -> Glance {
        Glance(waiting: waiting, answering: ["laptop"], hasMachines: true)
    }
}
