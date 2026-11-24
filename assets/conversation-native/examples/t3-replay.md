[
  {
    "name": "Working silently",
    "automatic": true,
    "turn": "{\"run\":null,\"requests\":[],\"lastMessage\":\"Fix the duplicate offline messages.\",\"replies\":\"\"}",
    "silent": true
  },
  {
    "name": "Finished result",
    "automatic": true,
    "turn": "{\"run\":{\"id\":\"one\",\"status\":\"completed\"},\"requests\":[],\"replies\":\"Fixed duplicate offline messages by retaining each message ID across retries. The retry handler previously assigned a fresh ID after reconnection. Modified queue.ts and recovery.test.ts. All 18 recovery tests passed locally. CI has not run. No deployment was performed.\"}"
  },
  {
    "name": "Ask for detail",
    "turn": "Why did it happen?",
    "sends": 0
  },
  {
    "name": "Direct the attached thread",
    "turn": "Have it test switching chats while offline too.",
    "sends": 1
  },
  {
    "name": "Same result on reconnect",
    "automatic": true,
    "turn": "{\"run\":{\"id\":\"one\",\"status\":\"completed\"},\"requests\":[],\"replies\":\"Fixed duplicate offline messages by retaining each message ID across retries. All 18 recovery tests passed locally. CI has not run.\"}",
    "silent": true
  },
  {
    "name": "Decision needed",
    "automatic": true,
    "turn": "{\"run\":{\"id\":\"two\",\"status\":\"completed\"},\"requests\":[],\"replies\":\"The cross-chat test passes. I found that two tabs can submit the same queued item at once. Should I add coordination between tabs now, or keep this change limited to reconnect retries?\"}"
  },
  {
    "name": "Answer the decision",
    "turn": "Keep it limited to reconnect retries.",
    "sends": 1
  },
  {
    "name": "Structured question",
    "automatic": true,
    "turn": "{\"run\":null,\"requests\":[{\"id\":\"scope-request\",\"kind\":\"user_input\",\"item\":{\"type\":\"user_input_request\",\"requestId\":\"scope-request\",\"questions\":[{\"id\":\"scope\",\"question\":\"Confirm scope for the next test run\",\"options\":[{\"label\":\"Reconnect retries only\"},{\"label\":\"Include cross-tab coordination\"}]}]}}],\"replies\":\"\"}"
  },
  {
    "name": "Answer structured question",
    "turn": "Reconnect retries only.",
    "sends": 1
  },
  {
    "name": "Ask for quiet",
    "turn": "Stay quiet unless you need a decision from me.",
    "sends": 0
  },
  {
    "name": "Respect quiet",
    "automatic": true,
    "turn": "{\"run\":{\"id\":\"three\",\"status\":\"completed\"},\"requests\":[],\"replies\":\"Done. Kept the change limited to reconnect retries. All tests passed.\"}",
    "silent": true
  },
  {
    "name": "Approval still needs attention",
    "automatic": true,
    "turn": "{\"run\":null,\"requests\":[{\"id\":\"approval\",\"kind\":\"command\",\"item\":{\"text\":\"Approval required in T3 to deploy to production.\"}}],\"replies\":\"\"}",
    "sends": 0
  },
  {
    "name": "Uncertain send is not repeated",
    "turn": "Tell it to stop after the tests, without deploying.",
    "sends": 1,
    "uncertain": true
  }
]
