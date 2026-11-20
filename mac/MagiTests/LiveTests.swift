import Foundation
import Testing

func fixture(_ path: String) throws -> Data {
    try Data(contentsOf: URL(filePath: #filePath).deletingLastPathComponent().appending(path: "Fixtures/\(path)"))
}

struct LiveTests {
    @Test func readsALiveFile() throws {
        let machine = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/laptop.json"))
        #expect(machine.answering)
        #expect(machine.heard?.timeIntervalSince1970 == 1795193133.936688)
        #expect(machine.cores == 10 && machine.load == 2.36)
        #expect(machine.memory.usedMB == 12567 && machine.memory.totalMB == 16384)
        #expect(machine.disk.usedMB == 281527 && machine.disk.totalMB == 471482)
        let waiting = machine.sessions[0]
        #expect(waiting.id == "07f37932-4bda-4e5a-a489-719326b0f477" && waiting.title == "Shop checkout fix")
        #expect(waiting.folder == "/Users/sam/code/shop" && waiting.state == "waiting" && waiting.waitingOn == "permission")
        #expect(waiting.since?.timeIntervalSince1970 == 1795193063)
        #expect(waiting.cpu == 0 && waiting.memoryMB == 229)
        #expect(machine.sessions[1].waitingOn == nil)
    }

    @Test func readsTheMovesInHandoffs() throws {
        let moves = Move.recent(in: try fixture("handoffs.jsonl"))
        #expect(moves.count == 4)
        #expect(moves[3].note?.hasPrefix("Moving session 3fce93f1 to box was interrupted.") == true)
        #expect(moves[0].from == "laptop" && moves[0].to == "box" && moves[0].toDir == "/home/sam/code/shop" && moves[0].note == nil)
        #expect(abs(moves[0].time.timeIntervalSince1970 - 1795192998.75481) < 0.000_001)
    }

    @Test func stopsAnsweringWhenQuietForFortySecondsOrSaysSo() throws {
        let laptop = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/laptop.json"))
        let box = try JSONDecoder.magi.decode(Machine.self, from: fixture("live/box.json"))
        let heard = try #require(laptop.heard)
        #expect(laptop.answers(at: heard.addingTimeInterval(40)))
        #expect(!laptop.answers(at: heard.addingTimeInterval(40.001)))
        #expect(!box.answers(at: try #require(box.heard)))
    }
}
