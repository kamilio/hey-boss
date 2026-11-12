# Recovery flow

Follow the saved request through each service. Expand the diagram to see the full route.

```mermaid
flowchart LR
 user[You send a message] --> chat[Chat saves your message]
 chat --> observer[Reply tracker saves a delivery task]
 observer --> session[Session saves agent history]
 session --> runner[Runner coordinates work]
 runner --> model[Model service calls provider]
 runner --> tools[Tool service performs actions]
 model --> saved[Saved model and tool results]
 tools --> saved
 saved --> session
 session --> observer
 observer --> outbox[Delivery queue keeps reply until confirmed]
 outbox --> chat
 chat --> user
```

## Recovery decision

```mermaid
flowchart TD
 A[Request interrupted] --> B{Result saved?}
 B -->|Yes| C[Replay the same result]
 B -->|No| D[Report incomplete work]
 C --> E[Confirm delivery]
 D --> E
```

## Broken source stays available

```mermaid
flowchart LR
 A[Broken
```

## Ordinary code

```js
const saved = true;
```

## Untrusted labels

```mermaid
flowchart LR
 A["<script>window.diagramPwned=1</script>"] --> B[Safe]
 click B "javascript:window.diagramPwned=2"
```