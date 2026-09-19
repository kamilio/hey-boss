## Delivery review

Proper **bold**, *emphasis*, ~~strikethrough~~ and `inline code`.

PRs:
- https://github.com/poe-internal/poe2/pull/14920
- https://github.com/poe-internal/poe2/pull/14921

See (https://example.com/a_(b)). Contact agent@example.com or visit www.example.com.
Query: https://example.com/?first=1&second=2.
An [explicit link](https://example.com/review "Review") and a [reference][review].

### Acceptance criteria

- [x] Preserve the accepted delivery
- [ ] Recover after reconnecting
  - Keep the original message order
  - Preserve **attached files**

3. Start the producer.
4. Disconnect the receiver.
   1. Wait for completion.
   2. Reconnect.

| Feature | State | Count |
| :--- | :---: | ---: |
| Chat replies | Ready | 12 |
| Attached files | Testing | 4 |

> [!NOTE]
> The original producer owns delivery.

> [!TIP]
> Check the durable journal first.

> [!IMPORTANT]
> Preserve the original ordering.

> [!WARNING]
> A timeout does not mean the producer stopped.

> [!CAUTION]
> Do not discard accepted work.

### Code stays literal

```typescript
const endpoint = "https://example.com/api";
// Never turn a URL inside code into a link.
await recover({ accepted: true, attempts: 3 });
```

```diff
- discard(reply);
+ await deliver(reply);
```

`https://inline.example.com` stays code.

    https://indented.example.com stays code too.

### References

A reference to the delivery contract.[^contract]

[^contract]: Accepted work remains owned until completion.

[review]: https://example.com/review

#### Fourth-level heading

##### Fifth-level heading

###### Sixth-level heading

Text with a hard break.  
Second line with Unicode: 日本語 · café · 🌍.

---

End of review.
