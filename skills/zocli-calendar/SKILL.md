---
name: zocli-calendar
description: "Zoho Calendar workflow: list calendars, inspect events, create meetings, and delete events. Use it for scheduling questions, availability checks, and event management."
metadata:
  author: NextStat
---

# zocli calendar

Read `zocli-shared` first if account setup or auth state is unclear.

## Commands

### List calendars

```
zocli calendar calendars [--profile ALIAS]
```

Returns the available calendars with their names and IDs.

### List events

```
zocli calendar events [FROM] [TO] [--calendar CALENDAR_UID] [--limit N] [--profile ALIAS]
```

`FROM` and `TO` accept `YYYY-MM-DD`. Without explicit bounds, `zocli` defaults to the next 30 days.

Example:
```
zocli calendar events 2026-03-16 2026-03-23 --limit 20
```

### Create an event

```
zocli calendar create TITLE START END [--calendar CALENDAR_UID] [--description TEXT] [--location PLACE] [--profile ALIAS]
```

`START` and `END` use RFC3339 timestamps such as `2026-03-16T09:00:00Z`.

Example:
```
zocli calendar create "Team Sync" 2026-03-16T09:00:00Z 2026-03-16T10:00:00Z --description "Weekly sync"
```

### Delete an event

```
zocli calendar delete UID [--calendar CALENDAR_UID] [--profile ALIAS]
```

Use the event UID returned by `calendar events`.

## Typical flow

1. Run `zocli calendar calendars` to discover the target calendar.
2. Run `zocli calendar events` with the narrowest useful window.
3. Run `zocli calendar create ...` or `zocli calendar delete ...`.
