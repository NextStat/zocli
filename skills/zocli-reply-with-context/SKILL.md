---
name: zocli-reply-with-context
description: "Reply to a message with calendar context: read the mail, inspect schedule constraints, then draft or send a reply. Use it when availability or timing matters."
metadata:
  author: NextStat
---

# Reply with calendar context

Multi-step workflow: read, check the calendar, then reply.

## Steps

1. Read the original message:

```
zocli mail read --folder-id FOLDER_ID MESSAGE_ID
```

2. Extract the dates or time windows mentioned in the mail.

3. Check the calendar:

```
zocli calendar events FROM TO
```

4. Reply with the relevant availability context:

```
zocli mail reply --folder-id FOLDER_ID MESSAGE_ID "Reply text"
```

## Examples

Mail: "Can we meet on Thursday or Friday?"
-> inspect both days in `calendar events` -> reply with the free slots.

Mail: "Can you confirm the meeting on March 15 at 14:00?"
-> inspect the day -> reply with confirmation or a conflict note.

## Notes

- Thread headers are handled automatically by `mail reply`.
- Use `--cc` or `--html` only when the reply genuinely needs them.
- State explicitly whether you actually sent the reply or only drafted one.
