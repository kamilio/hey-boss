import Foundation

struct RecentCommands {
    var defaults: UserDefaults = .standard
    private var recordings: [[String]] { defaults.array(forKey: "recentRecordingCommands") as? [[String]] ?? [] }

    // Reserve the slot at speech onset, including recordings that fail or are cancelled.
    func beginRecording() -> [[String]] {
        let previous = recordings
        defaults.set(Array(previous.suffix(4)) + [[]], forKey: "recentRecordingCommands")
        return previous.allSatisfy(\.isEmpty) ? [] : previous
    }

    func executed(_ action: CorrectionAction) {
        let name: String
        switch action {
        case .addVocabularyTerm: name = "add_dictation_vocabulary_term"
        case .paste: return
        case .nextThread: name = "t3_next_thread"
        case .switchThread(let position): name = "t3_switch_thread \(position)"
        case .listThreads: name = "t3_list_threads"
        case .newThread: name = "t3_new_thread"
        case .rename: name = "t3_rename_thread"
        case .pinThread(let pin): name = pin ? "t3_pin_thread" : "t3_unpin_thread"
        case .settleThread: name = "t3_settle_thread"
        case .clearInput: name = "clear_input"
        case .slack(.users): name = "slack_list_users"
        case .slack(.chat): name = "slack_open_chat"
        case .slack(.channel): name = "slack_open_channel"
        }
        var slots = recordings
        guard !slots.isEmpty else { return }
        slots[slots.count - 1].append(name)
        defaults.set(slots, forKey: "recentRecordingCommands")
    }
}
