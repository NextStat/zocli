---
name: zocli-daily-briefing
description: "Summarize recent mail and the upcoming schedule for one account. Works for English and Russian requests: morning brief, weekly digest, or a quick status check. Также используй для русских запросов на ежедневную сводку."
metadata:
  author: NextStat
---

# Daily briefing

Use this workflow for both English and Russian briefing requests.
Используй этот workflow и для английских, и для русских запросов на сводку.

This is a multi-step workflow for summarizing inbox activity and calendar events for a chosen period.

Это multi-step workflow для сводки по inbox и событиям календаря за выбранный период.

## Steps

1. Choose the window:
   - "today" -> `FROM=today`, `TO=tomorrow`, `limit=20`
   - "this week" -> `FROM=today`, `TO=today+7d`, `limit=50`
   - "this month" -> `FROM=today`, `TO=today+30d`, `limit=100`
   - if unspecified, default to a short operational window

2. Inspect recent mail:

```
zocli mail list --limit N
```

3. Inspect the calendar for the same period:

```
zocli calendar events FROM TO
```

4. If any message looks important, read it in full:

```
zocli mail read --folder-id FOLDER_ID MESSAGE_ID
```

Typical signals: urgent subjects, replies to your messages, calendar-related threads, or messages from key stakeholders.

5. Produce a short operational summary:
   - new or unread mail worth attention;
   - the next important events;
   - anything that requires a reply, follow-up, or scheduling action.
6. Match the user's language in the final summary. If the user writes in Russian, the summary should also be in Russian.

## Notes

- Use `--profile ALIAS` when the current account is not the right one.
- Increase `--limit` only when the inbox volume justifies it.
- Keep the final answer short and action-oriented.
