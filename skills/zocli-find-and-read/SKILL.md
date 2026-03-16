---
name: zocli-find-and-read
description: "Find and read a mail message for English and Russian requests: search by keyword, pick the best hit, then open the message by folder and message ID. Также используй для русских запросов, когда нужно найти конкретное письмо."
metadata:
  author: NextStat
---

# Find and read a message

Use this workflow for both English and Russian requests to locate a specific email.
Используй этот workflow и для английских, и для русских запросов на поиск конкретного письма.

Two-step workflow: search, then read.

Двухшаговый workflow: сначала поиск, потом чтение.

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
- For Russian requests, explain the chosen result in Russian but keep `folder_id` and `message_id` exactly as returned.
