---
name: zocli-find-and-read
description: "Find and read a mail message: search by keyword, pick the best hit, then open the message by folder and message ID. Use it when the user needs a specific email."
metadata:
  author: NextStat
---

# Find and read a message

Two-step workflow: search, then read.

## Steps

1. Search for the message:

```
zocli mail search "QUERY" --limit 5
```

2. Choose the best result and capture both the `folder_id` and the message `id`.

3. Read it:

```
zocli mail read --folder-id FOLDER_ID MESSAGE_ID
```

## Notes

- Start with a narrow query when the sender or subject is known.
- If there are too many matches, refine the query instead of reading multiple messages blindly.
- Always report which folder and message ID you selected.
